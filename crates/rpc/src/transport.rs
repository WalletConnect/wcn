use {
    crate::{BorrowedMessage, Message},
    arc_swap::ArcSwap,
    bytes::{BufMut as _, Bytes, BytesMut},
    futures_timer::Delay,
    governor::clock::ReasonablyRealtime as _,
    pin_project::pin_project,
    serde::{Deserialize, Serialize},
    std::{
        error::Error as StdError,
        future::Future as _,
        io,
        num::NonZeroU32,
        pin::Pin,
        sync::Arc,
        task::{self, Poll},
    },
    tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec},
};

mod sink;
mod stream;

/// Tranport priority. Transports with higher priority take precedence during
/// network congestion.
#[derive(Clone, Copy, Debug)]
pub enum Priority {
    High,
    Low,
}

/// Serialization codec.
pub trait Codec<M: Message>: Default + Serializer<M> + Deserializer<M> {}

impl<M: Message, C> Codec<M> for C where C: Default + Serializer<M> + Deserializer<M> {}

pub trait Serializer<M: BorrowedMessage>:
    tokio_serde::Serializer<M, Error: StdError> + Unpin + Send + Sync + 'static
{
}

pub trait Deserializer<M: Message>:
    tokio_serde::Deserializer<M, Error: StdError> + Unpin + Send + Sync + 'static
{
}

impl<M: BorrowedMessage, S> Serializer<M> for S where
    S: tokio_serde::Serializer<M, Error: StdError> + Unpin + Send + Sync + 'static
{
}

impl<M: Message, S> Deserializer<M> for S where
    S: tokio_serde::Deserializer<M, Error: StdError> + Unpin + Send + Sync + 'static
{
}

#[derive(Clone, Copy, Debug, Default)]
pub struct JsonCodec;

impl<T> tokio_serde::Deserializer<T> for JsonCodec
where
    for<'a> T: Deserialize<'a>,
{
    type Error = serde_json::Error;

    fn deserialize(self: Pin<&mut Self>, src: &BytesMut) -> Result<T, Self::Error> {
        serde_json::from_slice(src)
    }
}

impl<T> tokio_serde::Serializer<T> for JsonCodec
where
    T: Serialize,
{
    type Error = serde_json::Error;

    fn serialize(self: Pin<&mut Self>, data: &T) -> Result<Bytes, Self::Error> {
        serde_json::to_vec(data).map(Into::into)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PostcardCodec;

impl<T> tokio_serde::Deserializer<T> for PostcardCodec
where
    for<'a> T: Deserialize<'a>,
{
    type Error = io::Error;

    fn deserialize(self: Pin<&mut Self>, src: &BytesMut) -> Result<T, Self::Error> {
        postcard::from_bytes(src).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }
}

impl<T> tokio_serde::Serializer<T> for PostcardCodec
where
    T: Serialize,
{
    type Error = io::Error;

    fn serialize(self: Pin<&mut Self>, data: &T) -> Result<Bytes, Self::Error> {
        postcard::experimental::serialized_size(data)
            .and_then(|size| postcard::to_io(data, BytesMut::with_capacity(size).writer()))
            .map(|writer| writer.into_inner().freeze())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }
}

/// Untyped bi-directional stream.
pub(crate) struct BiDirectionalStream {
    pub rx: RecvStream,
    pub tx: SendStream,
}

pub(crate) type SendStream = FramedWrite<Throttled<quinn::SendStream>, LengthDelimitedCodec>;
pub(crate) type RecvStream = FramedRead<Throttled<quinn::RecvStream>, LengthDelimitedCodec>;

impl BiDirectionalStream {
    pub fn new(tx: quinn::SendStream, rx: quinn::RecvStream) -> Self {
        let tx = Throttled::new(tx, BandwidthLimiter::new(0));
        let rx = Throttled::new(rx, BandwidthLimiter::new(0));

        Self {
            tx: FramedWrite::new(tx, LengthDelimitedCodec::new()),
            rx: FramedRead::new(rx, LengthDelimitedCodec::new()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BandwidthLimiter {
    inner: Arc<ArcSwap<Option<BandwidthLimiterInner>>>,
}

#[derive(Debug)]
struct BandwidthLimiterInner {
    governor: governor::DefaultDirectRateLimiter,
    bytes_per_second: u32,
}

impl BandwidthLimiter {
    /// Creates a new [`BandwidthLimiter`] with the specified bytes per second
    /// limit.
    ///
    /// If `bytes_per_second` is `0` the [`BandwidthLimiter`] will be disabled.
    pub fn new(bytes_per_second: u32) -> Self {
        let this = Self {
            inner: Default::default(),
        };

        if bytes_per_second == 0 {
            return this;
        };

        this.set_bps(bytes_per_second);
        this
    }

    /// Gets the current bytes per second limit of this [`BandwidthLimiter`].
    ///
    /// `0` means that limiting is disabled.
    pub fn bps(&self) -> u32 {
        Option::as_ref(&self.inner.load())
            .map(|inner| inner.bytes_per_second)
            .unwrap_or_default()
    }

    /// Sets maximum bytes per second limit of this [`BandwidthLimiter`].
    ///
    /// Providing `0` disables the limiting.
    pub fn set_bps(&self, bytes_per_second: u32) {
        if self.bps() == bytes_per_second {
            return;
        }

        let governor = if bytes_per_second == 0 {
            None
        } else {
            // NOTE(unwrap): we just checked that it's not `0`
            let bps = NonZeroU32::new(bytes_per_second).unwrap();
            let quota = governor::Quota::per_second(bps);

            Some(governor::DefaultDirectRateLimiter::direct(quota))
        };

        self.inner
            .store(Arc::new(governor.map(|governor| BandwidthLimiterInner {
                governor,
                bytes_per_second,
            })));
    }
}

#[pin_project(project = ThrottledProj)]
pub(crate) struct Throttled<T> {
    #[pin]
    inner: T,

    limiter: BandwidthLimiter,
    delay: Option<Delay>,
    balance: isize,
}

impl<T> Throttled<T> {
    fn new(inner: T, limiter: BandwidthLimiter) -> Self {
        Self {
            inner,
            limiter,
            delay: None,
            balance: 0,
        }
    }

    pub(crate) fn set_limiter(&mut self, limiter: BandwidthLimiter) {
        self.limiter = limiter;
    }

    pub(crate) fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<'a, T> ThrottledProj<'a, T> {
    /// Tries to reserve the requested `amount` of byte credits for later use.
    ///
    /// Blocks ([`Poll::Pending`]) if the credits cannot be acquired from the
    /// underlying [`BandwidthLimiter`] immediately.
    ///
    /// If the credit balance is negative it will try to "repay" it.
    fn poll_reserve(&mut self, cx: &mut task::Context<'_>, amount: usize) -> Poll<usize> {
        let limiter = self.limiter.inner.load();

        let Some(limiter) = limiter.as_ref() else {
            return Poll::Ready(amount);
        };

        let amount = isize::try_from(amount).unwrap_or(isize::MAX);

        let mut amount = if *self.balance >= amount {
            // NOTE(unwrap): `amount` is always a positive number
            return Poll::Ready(usize::try_from(amount).unwrap());
        } else {
            let deficit = u32::try_from(self.balance.abs_diff(amount)).unwrap_or(u32::MAX);
            // NOTE(unwrap): `deficit` cannot be zero as `balance` < `amount` in this branch
            NonZeroU32::new(deficit).unwrap()
        };

        loop {
            if let Some(delay) = self.delay {
                match Pin::new(delay).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(_) => *self.delay = None,
                }
            };

            match limiter.governor.check_n(amount) {
                Ok(Ok(())) => {
                    *self.balance += isize::try_from(u32::from(amount)).unwrap_or(isize::MAX);
                    return Poll::Ready(usize::try_from(*self.balance).unwrap_or_default());
                }

                Ok(Err(negative)) => {
                    let now = limiter.governor.clock().reference_point();
                    *self.delay = Some(Delay::new(negative.wait_time_from(now)));
                }

                Err(governor::InsufficientCapacity(capacity)) => {
                    // NOTE(unwrap): `capacity` cannot be zero
                    amount = NonZeroU32::new(capacity).unwrap();
                }
            };
        }
    }

    /// Spends the specified `amount` of byte credits.
    fn spend(&mut self, amount: usize) {
        *self.balance = self
            .balance
            .saturating_sub(isize::try_from(amount).unwrap_or(isize::MAX));
    }
}
