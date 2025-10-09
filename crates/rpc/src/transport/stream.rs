use {
    crate::BandwidthLimiter,
    futures_timer::Delay,
    governor::{clock::ReasonablyRealtime, InsufficientCapacity},
    std::{
        future::Future as _,
        io,
        num::NonZeroU32,
        pin::Pin,
        task::{Context, Poll},
    },
    tokio::io::{AsyncRead, ReadBuf},
};

enum State {
    Wait,
    Ready,
}

pub struct ThrottledStream<T> {
    inner: T,
    delay: Delay,
    state: State,
    limiter: Option<BandwidthLimiter>,
    remaining_data: u32,
}

impl<T> ThrottledStream<T>
where
    T: AsyncRead + Unpin,
{
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            delay: Delay::new(Default::default()),
            state: State::Ready,
            limiter: None,
            remaining_data: 0,
        }
    }

    pub fn set_limiter(&mut self, limiter: Option<BandwidthLimiter>) {
        self.limiter = limiter;
    }

    fn update_delay(&mut self) {
        let Some(limiter) = self.limiter.as_ref() else {
            return;
        };

        let burst = limiter.burst();
        let limiter = limiter.inner();

        while self.remaining_data > 0 {
            let n = self.remaining_data.min(burst.get());

            // Safe unwrap, `n` can't be 0.
            let n = NonZeroU32::new(n).unwrap();

            match limiter.check_n(n) {
                Ok(Ok(())) => {
                    let n = n.get();

                    if self.remaining_data >= n {
                        self.remaining_data -= n;
                    } else {
                        self.remaining_data = 0;
                    }
                }

                Ok(Err(negative)) => {
                    let now = limiter.clock().reference_point();

                    self.delay.reset(negative.wait_time_from(now));
                    self.state = State::Wait;

                    return;
                }

                Err(InsufficientCapacity(_)) => {
                    // Unreachable, since `n` can't be greater than `burst`.
                    unreachable!();
                }
            };
        }

        self.state = State::Ready;
    }
}

#[cfg(test)]
impl<T> ThrottledStream<T> {
    pub fn with_limiter(mut self, limiter: BandwidthLimiter) -> Self {
        self.limiter = Some(limiter);
        self
    }
}

impl<T> AsyncRead for ThrottledStream<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            match self.state {
                State::Wait => match Pin::new(&mut self.delay).poll(cx) {
                    Poll::Pending => {
                        return Poll::Pending;
                    }

                    Poll::Ready(_) => {
                        self.update_delay();
                    }
                },

                State::Ready => {
                    let buf_len = buf.filled().len();

                    match Pin::new(&mut self.inner).poll_read(cx, buf) {
                        Poll::Pending => {
                            return Poll::Pending;
                        }

                        Poll::Ready(res) => match res {
                            Ok(()) => {
                                let bytes_read = buf.filled().len() - buf_len;

                                if bytes_read > 0 {
                                    self.remaining_data = bytes_read as u32;
                                    self.update_delay();
                                }

                                return Poll::Ready(Ok(()));
                            }

                            Err(err) => {
                                return Poll::Ready(Err(err));
                            }
                        },
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use {
        super::*,
        std::{
            io::Cursor,
            time::{Duration, Instant},
        },
        tokio::io::AsyncReadExt,
    };

    #[tokio::test]
    async fn single() {
        let expected = vec![1u8; 35];
        let limiter = BandwidthLimiter::new(5);
        let reader = Cursor::new(expected.clone());
        let mut stream = ThrottledStream::new(reader).with_limiter(limiter);

        let time = Instant::now();
        let mut actual = Vec::new();
        let bytes_read = stream.read_to_end(&mut actual).await.unwrap();

        let elapsed = time.elapsed();

        // Expect 6.0s actual time, because of the allowed burst at the beginning.
        assert!(elapsed >= Duration::from_millis(6000));
        assert!(elapsed < Duration::from_millis(6200));
        assert_eq!(bytes_read, expected.len());
        assert_eq!(expected, actual);
    }

    #[tokio::test]
    async fn multiple() {
        let expected1 = vec![1u8; 35];
        let expected2 = vec![2u8; 35];
        let expected3 = vec![3u8; 35];
        let limiter = BandwidthLimiter::new(10);

        let reader1 = Cursor::new(expected1.clone());
        let reader2 = Cursor::new(expected2.clone());
        let reader3 = Cursor::new(expected3.clone());

        let mut stream1 = ThrottledStream::new(reader1).with_limiter(limiter.clone());
        let mut stream2 = ThrottledStream::new(reader2).with_limiter(limiter.clone());
        let mut stream3 = ThrottledStream::new(reader3).with_limiter(limiter);

        let time = Instant::now();

        let mut actual1 = Vec::new();
        let mut actual2 = Vec::new();
        let mut actual3 = Vec::new();

        let (res1, res2, res3) = tokio::join!(
            stream1.read_to_end(&mut actual1),
            stream2.read_to_end(&mut actual2),
            stream3.read_to_end(&mut actual3)
        );

        let elapsed = time.elapsed();

        // Expect 9.5s actual time, because of the allowed burst at the beginning.
        assert!(elapsed >= Duration::from_millis(9500));
        assert!(elapsed < Duration::from_millis(9700));
        assert_eq!(res1.unwrap(), expected1.len());
        assert_eq!(res2.unwrap(), expected2.len());
        assert_eq!(res3.unwrap(), expected3.len());
        assert_eq!(expected1, actual1);
        assert_eq!(expected2, actual2);
        assert_eq!(expected3, actual3);
    }
}
