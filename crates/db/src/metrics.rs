use {
    crate::Config,
    std::{future::Future, io, path::Path},
    tap::{TapFallible as _, TapOptional as _},
    wcn_rocks::RocksBackend,
};

pub(super) fn prometheus_server(cfg: &Config) -> io::Result<impl Future<Output = ()>> {
    let socket = cfg.metrics_server_socket.try_clone()?;
    let prometheus = cfg.prometheus_handle.clone();
    let shutdown_fut = cfg.shutdown_signal.wait_owned();

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
            .with_graceful_shutdown(shutdown_fut)
            .await;
    })
}

pub(super) fn custom_system_monitor_metrics_fn(
    cfg: &Config,
    db: RocksBackend,
) -> impl Fn() + Send + 'static {
    let is_rocksdb_metrics_enabled = cfg.rocksdb.enable_metrics;
    let db = db.clone();

    move || {
        // We have a similar issue to https://github.com/facebook/rocksdb/issues/3889
        // PhysicalCoreID() consumes 5-10% CPU, so for now rocksdb metrics are behind a
        // flag.
        if is_rocksdb_metrics_enabled {
            update_rocksdb_metrics(&db);
        }
    }
}

pub(super) fn update_rocksdb_metrics(db: &RocksBackend) {
    match db.memory_usage() {
        Ok(s) => {
            metrics::gauge!("wcn_rocksdb_mem_table_total").set(s.mem_table_total as f64);
            metrics::gauge!("wcn_rocksdb_mem_table_unflushed").set(s.mem_table_unflushed as f64);
            metrics::gauge!("wcn_rocksdb_mem_table_readers_total",)
                .set(s.mem_table_readers_total as f64);
            metrics::gauge!("wcn_rocksdb_cache_total").set(s.cache_total as f64);
        }

        Err(err) => tracing::warn!(?err, "failed to get rocksdb memory usage stats"),
    }

    let stats = match db.statistics() {
        Ok(Some(stats)) => stats,
        Ok(None) => {
            tracing::warn!("rocksdb statistics are disabled");
            return;
        }
        Err(err) => {
            tracing::warn!(?err, "failed to get rocksdb statistics");
            return;
        }
    };

    for (name, stat) in stats {
        let name = format!("wcn_{}", name.replace('.', "_"));

        match stat {
            wcn_rocks::db::Statistic::Ticker(count) => {
                metrics::counter!(name).increment(count);
            }

            wcn_rocks::db::Statistic::Histogram(h) => {
                // The distribution is already calculated for us by RocksDB, so we use
                // `gauge`/`counter` here instead of `histogram`.

                metrics::counter!(format!("{name}_count")).increment(h.count);
                metrics::counter!(format!("{name}_sum")).increment(h.sum);

                metrics::gauge!(name.clone(), "p" => "50").set(h.p50);
                metrics::gauge!(name.clone(), "p" => "95").set(h.p95);
                metrics::gauge!(name.clone(), "p" => "99").set(h.p99);
                metrics::gauge!(name, "p" => "100").set(h.p100);
            }
        }
    }
}

pub(super) fn rocksdb_mount_point(cfg: &Config) -> &Path {
    let path = &cfg.rocksdb_dir;

    // Try to detect the closest mount point. Fallback to the parent directory.
    find_storage_mount_point(path)
        .tap_none(|| tracing::warn!(?path, "failed to find mount point of rocksdb directory"))
        .or_else(|| path.parent())
        .unwrap_or(path)
}

fn find_storage_mount_point(mut path: &Path) -> Option<&Path> {
    let mounts = proc_mounts::MountList::new()
        .tap_err(|err| tracing::warn!(?err, "failed to read list of mounted file systems"))
        .ok()?;

    loop {
        if mounts.get_mount_by_dest(path).is_some() {
            return Some(path);
        }

        if let Some(parent) = path.parent() {
            path = parent;
        } else {
            return None;
        }
    }
}
