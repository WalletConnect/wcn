use {
    super::Throttled,
    std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    },
    tokio::io::{AsyncRead, ReadBuf},
};

impl<T> AsyncRead for Throttled<T>
where
    T: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut this = self.project();

        // If limiter is off, just poll `inner`
        let Some(max_buffer_size) = this.limiter_bps() else {
            return this.inner.poll_read(cx, buf);
        };

        let Poll::Ready(_) = this.poll_reserve(cx, 0) else {
            return Poll::Pending;
        };

        let mut new_buf = ReadBuf::new(if buf.remaining() >= max_buffer_size {
            buf.initialize_unfilled_to(max_buffer_size)
        } else {
            buf.initialize_unfilled()
        });

        if !this.inner.as_mut().poll_read(cx, &mut new_buf)?.is_ready() {
            return Poll::Pending;
        }

        let bytes_read = new_buf.filled().len();
        this.spend(bytes_read);
        buf.advance(bytes_read);

        Poll::Ready(Ok(()))
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
        tokio::io::AsyncReadExt,
    };

    #[tokio::test]
    async fn single() {
        let expected = vec![1u8; 35];
        let limiter = BandwidthLimiter::new(5);
        let reader = Cursor::new(expected.clone());
        let mut stream = Throttled::new(reader, Some(limiter));

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

        let mut stream1 = Throttled::new(reader1, Some(limiter.clone()));
        let mut stream2 = Throttled::new(reader2, Some(limiter.clone()));
        let mut stream3 = Throttled::new(reader3, Some(limiter));

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
