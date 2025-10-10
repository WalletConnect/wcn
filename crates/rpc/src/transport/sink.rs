use {
    super::Throttled,
    std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    },
    tokio::io::AsyncWrite,
};

impl<T> AsyncWrite for Throttled<T>
where
    T: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut this = self.project();

        let Poll::Ready(reserved) = this.poll_reserve(cx, buf.len()) else {
            return Poll::Pending;
        };

        let Poll::Ready(res) = this.inner.as_mut().poll_write(cx, &buf[..reserved]) else {
            return Poll::Pending;
        };

        // On error assume that the whole buffer has been written.
        let bytes_written = res.as_ref().copied().unwrap_or(buf.len());

        this.spend(bytes_written);

        Poll::Ready(res)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

#[cfg(test)]
mod test {
    use {
        super::*,
        crate::transport::BandwidthLimiter,
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
        let mut sink = Throttled::new(writer, Some(limiter));
        let expected = vec![1u8; 35];

        let time = Instant::now();
        sink.write_all(&expected[..]).await.unwrap();
        sink.flush().await.unwrap();

        let writer = sink.inner;
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

        let mut sink1 = Throttled::new(writer1, Some(limiter.clone()));
        let mut sink2 = Throttled::new(writer2, Some(limiter.clone()));
        let mut sink3 = Throttled::new(writer3, Some(limiter));

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
        assert_eq!(sink1.inner.into_inner(), expected1);
        assert_eq!(sink2.inner.into_inner(), expected2);
        assert_eq!(sink3.inner.into_inner(), expected3);
    }
}
