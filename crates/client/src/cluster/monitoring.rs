use {
    futures::StreamExt as _,
    phi_accrual_failure_detector::{Detector as _, SyncDetector},
    std::{
        collections::VecDeque,
        net::SocketAddrV4,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    },
    tap::Pipe as _,
    tokio_stream::wrappers::IntervalStream,
    tokio_util::sync::DropGuard,
    wc::{
        future::{CancellationToken, FutureExt as _},
        metrics::{self, FutureExt as _},
    },
    wcn_storage_api::{
        Namespace,
        StorageApi as _,
        operation as op,
        rpc::client::CoordinatorConnection,
    },
};

const HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(2500);

pub(super) struct ConnectionState {
    conn: CoordinatorConnection,
    ns: Namespace,
    detector: SyncDetector,
    latency: Mutex<LatencyHistory>,
}

impl ConnectionState {
    pub fn new(conn: CoordinatorConnection, ns: Namespace) -> Self {
        Self {
            conn,
            ns,
            detector: SyncDetector::default(),
            latency: Mutex::new(LatencyHistory::new(5)),
        }
    }

    pub fn remote_addr(&self) -> &SocketAddrV4 {
        self.conn.remote_peer_addr()
    }

    pub fn latency(&self) -> f64 {
        // Safe unwrap, as it can't panic.
        self.latency.lock().unwrap().mean()
    }

    pub fn suspicion_score(&self) -> f64 {
        self.detector.phi()
    }

    pub async fn heartbeat(&self) {
        let time = Instant::now();

        let op = op::GetBorrowed {
            namespace: self.ns,
            key: &[0],
            keyspace_version: None,
        };

        let conn = &self.conn;

        let res = async {
            conn.wait_open().await;
            conn.execute_ref(&op::Operation::Borrowed(op.into())).await
        }
        .with_timeout(HEARTBEAT_TIMEOUT)
        .await;

        // We're only interested in successful requests here. The errors will cause
        // missed heartbeats.
        if matches!(res, Ok(Ok(_))) {
            let elapsed = time.elapsed();

            // Safe unwrap, as it can't panic.
            self.latency.lock().unwrap().update(elapsed.as_secs_f64());
            self.detector.heartbeat();
        }
    }

    pub fn spawn_monitor(self: Arc<Self>) -> DropGuard {
        let token = CancellationToken::new();

        async move {
            let state = self.as_ref();
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            IntervalStream::new(interval)
                .for_each_concurrent(None, |_| state.heartbeat())
                .await;
        }
        .with_metrics(metrics::future_metrics!("wcn_connection_monitor"))
        .with_cancellation(token.clone())
        .pipe(tokio::spawn);

        token.drop_guard()
    }
}

pub(super) struct NodeState {
    public_conn: Arc<ConnectionState>,
    private_conn: Option<Arc<ConnectionState>>,
    _public_guard: DropGuard,
    _private_guard: Option<DropGuard>,
}

impl NodeState {
    pub fn new(
        public_conn: CoordinatorConnection,
        private_conn: Option<CoordinatorConnection>,
        ns: Namespace,
    ) -> Self {
        let public_conn = Arc::new(ConnectionState::new(public_conn, ns));
        let private_conn = private_conn.map(|conn| Arc::new(ConnectionState::new(conn, ns)));
        let _public_guard = public_conn.clone().spawn_monitor();
        let _private_guard = private_conn.clone().map(|state| state.spawn_monitor());

        Self {
            public_conn,
            private_conn,
            _public_guard,
            _private_guard,
        }
    }

    pub fn public_state(&self) -> &ConnectionState {
        &self.public_conn
    }

    pub fn private_state(&self) -> Option<&ConnectionState> {
        self.private_conn.as_ref().map(AsRef::as_ref)
    }
}

// Simple ring buffer to calculate mean latency over an arbitrary window.
struct LatencyHistory {
    data: VecDeque<f64>,
    sum: f64,
    capacity: usize,
}

impl LatencyHistory {
    fn new(capacity: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(capacity),
            sum: 0.0,
            capacity,
        }
    }

    fn update(&mut self, latency: f64) {
        if self.data.len() >= self.capacity
            && let Some(latency) = self.data.pop_front()
        {
            self.sum -= latency;
        }

        self.data.push_back(latency);
        self.sum += latency;
    }

    fn mean(&self) -> f64 {
        if self.data.is_empty() {
            0.0
        } else {
            self.sum / self.data.len() as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_history() {
        let mut hist = LatencyHistory::new(3);
        assert_eq!(hist.mean(), 0.0);
        hist.update(1.0);
        hist.update(2.0);
        hist.update(3.0);
        assert_eq!(hist.mean(), 2.0);
        hist.update(4.0);
        assert_eq!(hist.mean(), 3.0);
        hist.update(5.0);
        assert_eq!(hist.mean(), 4.0);
        hist.update(6.0);
        assert_eq!(hist.mean(), 5.0);
    }
}
