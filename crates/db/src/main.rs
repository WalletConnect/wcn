use {
    anyhow::Context as _,
    futures::{future::FusedFuture as _, FutureExt as _},
    std::pin,
    tokio::signal::unix::SignalKind,
    wc::metrics::exporter_prometheus::PrometheusBuilder,
    wcn_db::{config, Error},
};

#[global_allocator]
static GLOBAL: wc::alloc::Jemalloc = wc::alloc::Jemalloc;

fn main() -> anyhow::Result<()> {
    let _logger = wcn_logging::Logger::init(wcn_logging::LogFormat::Json, None, None);

    let prometheus = PrometheusBuilder::new()
        .install_recorder()
        .map_err(Error::Prometheus)?;

    for (key, value) in vergen_pretty::vergen_pretty_env!() {
        if let Some(value) = value {
            tracing::warn!(key, value, "build info");
        }
    }

    let version: f64 = include_str!("../../../VERSION")
        .trim_end()
        .parse()
        .map_err(|err| tracing::warn!(?err, "Failed to parse VERSION file"))
        .unwrap_or_default();
    wc::metrics::gauge!("wcn_db_version").set(version);

    let cfg = config::Config::from_env(prometheus).context("failed to parse config")?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let shutdown_signal = cfg.shutdown_signal.clone();
            let mut shutdown_fut = pin::pin!(tokio::signal::ctrl_c().fuse());

            let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate())?;

            let db_srv_fut = wcn_db::run(cfg)?;
            let mut db_srv_fut = pin::pin!(db_srv_fut.fuse());

            loop {
                tokio::select! {
                    biased;

                    _ = &mut shutdown_fut, if !shutdown_fut.is_terminated() => {
                        shutdown_signal.emit();
                    }

                    _ = sigterm.recv() => {
                        shutdown_signal.emit();
                    }

                    _ = &mut db_srv_fut => {
                        tracing::info!("database server stopped");
                        break;
                    }
                }
            }

            Ok(())
        })
}
