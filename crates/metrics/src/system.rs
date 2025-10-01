use {
    futures::FutureExt as _,
    futures_concurrency::future::Race as _,
    std::{path::PathBuf, time::Duration},
    sysinfo::{Disks, Networks, System},
    tokio::sync::mpsc,
    wc::metrics::{backend as metrics, exporter_prometheus::PrometheusHandle},
};

/// System monitor.
///
/// Collects system metrics and exports them.
pub struct Monitor<C = fn()> {
    prometheus_handle: PrometheusHandle,
    interval: Duration,
    disk_mount_point: Option<PathBuf>,
    custom_metrics: C,
}

impl Monitor {
    /// Creates a new system [`Monitor`].
    pub fn new(prometheus_handle: PrometheusHandle) -> Monitor {
        Monitor {
            prometheus_handle,
            interval: Duration::from_secs(15),
            disk_mount_point: None,
            custom_metrics: || {},
        }
    }
}

impl<C> Monitor<C> {
    /// Specified the interval of metrics collection/export.
    ///
    /// Default: 15s
    pub fn with_interval(mut self, period: Duration) -> Self {
        self.interval = period;
        self
    }

    /// Enables disk metrics for the specified `mount_point`.
    pub fn with_disk_metrics(self, mount_point: PathBuf) -> Self {
        Self {
            prometheus_handle: self.prometheus_handle,
            interval: self.interval,
            disk_mount_point: Some(mount_point),
            custom_metrics: self.custom_metrics,
        }
    }

    /// Sets a custom function that's going to be called on each tick of this
    /// system [`Monitor`].
    pub fn with_custom_metrics<F>(self, f: F) -> Monitor<F> {
        Monitor {
            prometheus_handle: self.prometheus_handle,
            interval: self.interval,
            disk_mount_point: self.disk_mount_point,
            custom_metrics: f,
        }
    }

    /// Runs this system [`Monitor`].
    pub async fn run(self, shutdown: impl Future)
    where
        C: Fn() + Send + 'static,
    {
        let shutdown = shutdown.map(drop);

        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let (tx, rx) = mpsc::channel(1);

        tokio::task::spawn_blocking(move || self.update_loop(rx));

        let fut = async {
            loop {
                interval.tick().await;

                if tx.send(()).await.is_err() {
                    tracing::warn!("Worker thread died");
                }
            }
        };

        (fut, shutdown).race().await
    }

    fn update_loop(self, mut ticks: mpsc::Receiver<()>)
    where
        C: Fn(),
    {
        let mut sys = sysinfo::System::new_all();

        loop {
            if ticks.blocking_recv().is_none() {
                return;
            }

            self.update_system(&mut sys);

            (self.custom_metrics)();

            self.prometheus_handle.run_upkeep();
        }
    }

    fn update_system(&self, sys: &mut System) {
        if let Err(err) = wc::alloc::stats::update_jemalloc_metrics() {
            tracing::warn!(?err, "failed to get jemalloc allocation stats");
        }

        update_cgroup_stats();

        sys.refresh_cpu_usage();

        for (n, cpu) in sys.cpus().iter().enumerate() {
            metrics::gauge!("wcn_cpu_usage_percent_per_core_gauge", "n_core" => n.to_string())
                .set(cpu.cpu_usage())
        }

        sys.refresh_memory();

        metrics::gauge!("wcn_total_memory").set(sys.total_memory() as f64);
        metrics::gauge!("wcn_free_memory").set(sys.free_memory() as f64);
        metrics::gauge!("wcn_available_memory").set(sys.available_memory() as f64);
        metrics::gauge!("wcn_used_memory").set(sys.used_memory() as f64);

        if let Some(path) = &self.disk_mount_point {
            let disks = Disks::new_with_refreshed_list();
            for disk in disks.list() {
                if disk.mount_point() == path {
                    metrics::gauge!("wcn_disk_total_space").set(disk.total_space() as f64);
                    metrics::gauge!("wcn_disk_available_space").set(disk.available_space() as f64);
                    break;
                }
            }
        }

        let networks = Networks::new_with_refreshed_list();
        for (name, net) in networks.list() {
            metrics::gauge!("wcn_network_tx_bytes_total", "network" => name.to_owned())
                .set(net.total_transmitted() as f64);
            metrics::gauge!("wcn_network_rx_bytes_total", "network" => name.to_owned())
                .set(net.total_received() as f64);
        }
    }
}

fn update_cgroup_stats() {
    // For details on the values see:
    //      https://www.kernel.org/doc/Documentation/cgroup-v2.txt
    const MEMORY_STAT_PATH: &str = "/sys/fs/cgroup/memory.stat";

    let Ok(data) = std::fs::read_to_string(MEMORY_STAT_PATH) else {
        return;
    };

    for line in data.lines() {
        let mut parts = line.split(' ');

        let (Some(stat), Some(val), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };

        let Ok(val) = val.parse::<f64>() else {
            continue;
        };

        metrics::gauge!("wcn_memory_stat", "stat" => stat.to_owned()).set(val);
    }
}
