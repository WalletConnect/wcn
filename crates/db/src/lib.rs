pub use config::Config;
use {
    crate::storage::Storage,
    futures::FutureExt as _,
    futures_concurrency::future::Join as _,
    metrics_exporter_prometheus::BuildError as PrometheusBuildError,
    std::{future::Future, io, time::Duration},
    tap::Pipe as _,
    wcn_rpc::server::{Api as _, Server},
};

pub mod metrics;
mod server;
mod storage;

pub mod config;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to initialize prometheus: {0:?}")]
    Prometheus(PrometheusBuildError),

    #[error("Failed to initialize storage: {0:?}")]
    Storage(#[from] storage::Error),

    #[error("Metrics server error: {0}")]
    MetricsServer(io::Error),

    #[error("Database server error: {0}")]
    DatabaseServer(#[from] wcn_rpc::server::Error),
}

pub fn run(cfg: Config) -> Result<impl Future<Output = ()> + Send, Error> {
    let id = cfg.id();

    tracing::info!(ports = ?[cfg.primary_rpc_server_socket.port(), cfg.secondary_rpc_server_socket.port()], %id, "starting database server");

    let storage = Storage::new(&cfg)?;

    let primary_rpc_server_cfg = wcn_rpc::server::Config {
        name: "primary",
        socket: cfg.primary_rpc_server_socket,
        keypair: cfg.keypair.clone(),
        connection_timeout: cfg.connection_timeout,
        max_connections: cfg.max_connections,
        max_connections_per_ip: cfg.max_connections_per_ip,
        max_connection_rate_per_ip: cfg.max_connection_rate_per_ip,
        max_concurrent_rpcs: cfg.max_concurrent_rpcs,
        shutdown_signal: cfg.shutdown_signal.clone(),
    };

    let secondary_rpc_server_cfg = wcn_rpc::server::Config {
        name: "secondary",
        socket: cfg.secondary_rpc_server_socket,
        keypair: cfg.keypair,
        connection_timeout: cfg.connection_timeout,
        max_connections: cfg.max_connections,
        max_connections_per_ip: cfg.max_connections_per_ip,
        max_connection_rate_per_ip: cfg.max_connection_rate_per_ip,
        max_concurrent_rpcs: cfg.max_concurrent_rpcs,
        shutdown_signal: cfg.shutdown_signal.clone(),
    };

    let metrics_server_cfg = metrics::ServerConfig {
        socket: cfg.metrics_server_socket,
        rocksdb_dir: cfg.rocksdb_dir.clone(),
        rocksdb: cfg.rocksdb.enable_metrics.then(|| storage.db().clone()),
        prometheus: cfg.prometheus_handle,
        shutdown_signal: cfg.shutdown_signal,
    };

    let database_api = wcn_storage_api::rpc::DatabaseApi::new()
        .with_rpc_timeout(Duration::from_millis(500))
        .with_state(server::Server::new(storage));

    let primary_rpc_server_fut = database_api
        .clone()
        .into_server()
        .serve(primary_rpc_server_cfg)?;

    let secondary_rpc_server_fut = database_api.into_server().serve(secondary_rpc_server_cfg)?;

    let metrics_server_fut = metrics::serve(metrics_server_cfg).map_err(Error::MetricsServer)?;

    (
        primary_rpc_server_fut,
        secondary_rpc_server_fut,
        metrics_server_fut,
    )
        .join()
        .map(drop)
        .pipe(Ok)
}
