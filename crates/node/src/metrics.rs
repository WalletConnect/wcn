use {
    metrics_exporter_prometheus::PrometheusHandle,
    std::{future::Future, io, net::TcpListener, time::Duration},
    sysinfo::Networks,
    wcn_metrics_api::MetricsApi,
    wcn_rpc::server::ShutdownSignal,
};

pub(super) fn serve(
    socket: TcpListener,
    prometheus: PrometheusHandle,
    shutdown_signal: ShutdownSignal,
) -> io::Result<impl Future<Output = ()>> {
    let shutdown_signal_ = shutdown_signal.clone();
    let prometheus_ = prometheus.clone();
    tokio::task::spawn_blocking(move || update_loop(prometheus_, shutdown_signal_));

    let svc = axum::Router::new()
        .route(
            "/metrics",
            axum::routing::get(move || async move { prometheus.render() }),
        )
        .into_make_service();

    socket.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(socket)?;

    Ok(async {
        let _ = axum::serve(listener, svc)
            .with_graceful_shutdown(async move { shutdown_signal.wait().await })
            .await;
    })
}

fn update_loop(prometheus: PrometheusHandle, shutdown_signal: ShutdownSignal) {
    let mut sys = sysinfo::System::new_all();

    loop {
        if shutdown_signal.is_emitted() {
            return;
        }

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

        let networks = Networks::new_with_refreshed_list();
        for (name, net) in networks.list() {
            metrics::gauge!("wcn_network_tx_bytes_total", "network" => name.to_owned())
                .set(net.total_transmitted() as f64);
            metrics::gauge!("wcn_network_rx_bytes_total", "network" => name.to_owned())
                .set(net.total_received() as f64);
        }

        prometheus.run_upkeep();
        std::thread::sleep(Duration::from_secs(15));
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
