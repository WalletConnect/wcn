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
    tokio::io::AsyncWrite,
};

enum State {
    NotReady,
    Wait,
    Ready,
}

pub struct ThrottledSink<T> {
    inner: T,
    delay: Delay,
    state: State,
    limiter: Option<BandwidthLimiter>,
}

impl<T> ThrottledSink<T>
where
    T: AsyncWrite + Unpin,
{
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            delay: Delay::new(Default::default()),
            state: State::NotReady,
            limiter: None,
        }
    }

    pub fn set_limiter(&mut self, limiter: Option<BandwidthLimiter>) {
        self.limiter = limiter;
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

#[cfg(test)]
impl<T> ThrottledSink<T> {
    pub fn with_limiter(mut self, limiter: BandwidthLimiter) -> Self {
        self.limiter = Some(limiter);
        self
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> AsyncWrite for ThrottledSink<T>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let Some(mut data_size) = NonZeroU32::new(buf.len() as u32) else {
            return Poll::Ready(Ok(0));
        };

        loop {
            match self.state {
                State::NotReady => {
                    let limiter = self.limiter.as_ref().map(|limiter| limiter.inner());

                    let Some(limiter) = limiter else {
                        self.state = State::Ready;
                        continue;
                    };

                    match limiter.check_n(data_size) {
                        Ok(Ok(())) => {
                            self.state = State::Ready;
                        }

                        Ok(Err(negative)) => {
                            let now = limiter.clock().reference_point();

                            self.delay.reset(negative.wait_time_from(now));

                            match Pin::new(&mut self.delay).poll(cx) {
                                Poll::Pending => {
                                    self.state = State::Wait;
                                    return Poll::Pending;
                                }

                                Poll::Ready(_) => {}
                            }
                        }

                        Err(InsufficientCapacity(cap)) => {
                            // Safe unwrap, as `cap` can't be 0.
                            data_size = NonZeroU32::new(cap).unwrap();
                        }
                    };
                }

                State::Wait => match Pin::new(&mut self.delay).poll(cx) {
                    Poll::Pending => {
                        return Poll::Pending;
                    }

                    Poll::Ready(_) => {
                        self.state = State::NotReady;
                    }
                },

                State::Ready => {
                    if self.limiter.is_some() {
                        self.state = State::NotReady;
                    }

                    let data_size = data_size.get() as usize;
                    let buf = &buf[..data_size];

                    return Pin::new(&mut self.inner).poll_write(cx, buf);
                }
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
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
        tokio::io::AsyncWriteExt,
    };

    #[tokio::test]
    async fn single() {
        let limiter = BandwidthLimiter::new(5);
        let writer = Cursor::new(vec![]);
        let mut sink = ThrottledSink::new(writer).with_limiter(limiter);
        let expected = vec![1u8; 35];

        let time = Instant::now();
        sink.write_all(&expected[..]).await.unwrap();
        sink.flush().await.unwrap();

        let writer = sink.into_inner();
        let actual = writer.into_inner();

        let elapsed = time.elapsed();

        // Expect 6.0s actual time, because of the allowed burst at the beginning.
        assert!(elapsed >= Duration::from_millis(6000));
        assert!(elapsed < Duration::from_millis(6200));
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn multiple() {
        let limiter = BandwidthLimiter::new(10);

        let expected1 = vec![1u8; 35];
        let expected2 = vec![2u8; 35];
        let expected3 = vec![3u8; 35];

        let writer1 = Cursor::new(vec![]);
        let writer2 = Cursor::new(vec![]);
        let writer3 = Cursor::new(vec![]);

        let mut sink1 = ThrottledSink::new(writer1).with_limiter(limiter.clone());
        let mut sink2 = ThrottledSink::new(writer2).with_limiter(limiter.clone());
        let mut sink3 = ThrottledSink::new(writer3).with_limiter(limiter);

        let time = Instant::now();

        let write_fut1 = async {
            sink1.write_all(&expected1[..]).await.unwrap();
            sink1.flush().await.unwrap();
        };
        let write_fut2 = async {
            sink2.write_all(&expected2[..]).await.unwrap();
            sink2.flush().await.unwrap();
        };
        let write_fut3 = async {
            sink3.write_all(&expected3[..]).await.unwrap();
            sink3.flush().await.unwrap();
        };

        tokio::join!(write_fut1, write_fut2, write_fut3);

        let elapsed = time.elapsed();

        // Expect 9.5s actual time, because of the allowed burst at the beginning.
        assert!(elapsed >= Duration::from_millis(9500));
        assert!(elapsed < Duration::from_millis(9700));
        assert_eq!(sink1.into_inner().into_inner(), expected1);
        assert_eq!(sink2.into_inner().into_inner(), expected2);
        assert_eq!(sink3.into_inner().into_inner(), expected3);
    }
}
