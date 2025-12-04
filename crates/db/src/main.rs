use {
    anyhow::Context as _,
    futures::FutureExt,
    wc::metrics::exporter_prometheus::PrometheusBuilder,
    wcn_db::{config, Error},
    wcn_rpc::server::run_with_signal_handling,
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
            let db_src_fut = wcn_db::run(cfg)?.map(|_| tracing::info!("database server stopped"));
            run_with_signal_handling(shutdown_signal, db_src_fut).await
        })
}
