use {
    anyhow::Context,
    base64::Engine as _,
    derive_where::derive_where,
    libp2p_identity::{Keypair, PeerId},
    metrics_exporter_prometheus::PrometheusHandle,
    serde::{Deserialize, Deserializer},
    std::{
        net::{Ipv4Addr, SocketAddrV4, TcpListener},
        path::PathBuf,
        time::Duration,
    },
    tap::{Pipe as _, TapOptional as _},
    wcn_rocks::RocksdbDatabaseConfig,
    wcn_rpc::server::ShutdownSignal,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ed25519 private key decoding failed")]
    KeyDecodingFailed,

    #[error("Invalid storage node address")]
    InvalidNodeAddress,
}

#[derive_where(Debug)]
pub struct Config {
    pub keypair: Keypair,

    pub primary_rpc_server_socket: wcn_rpc::server::Socket,
    pub secondary_rpc_server_socket: wcn_rpc::server::Socket,
    pub metrics_server_socket: TcpListener,

    pub connection_timeout: Duration,
    pub max_connections: u32,
    pub max_connections_per_ip: u32,
    pub max_connection_rate_per_ip: u32,
    pub max_concurrent_rpcs: u32,

    pub rocksdb_dir: PathBuf,
    pub rocksdb: RocksdbDatabaseConfig,

    pub shutdown_signal: ShutdownSignal,

    /// [`PrometheusHandle`] to use for getting metrics in metrics server.
    #[derive_where(skip)]
    pub prometheus_handle: PrometheusHandle,
}

impl Config {
    pub fn id(&self) -> PeerId {
        PeerId::from_public_key(&self.keypair.public())
    }
}

impl Config {
    pub fn from_env(prometheus_handle: PrometheusHandle) -> anyhow::Result<Self> {
        let raw = envy::from_env::<RawConfig>()?;
        let rocksdb = create_rocksdb_config(&raw);

        tracing::info!(config = ?rocksdb, "rocksdb configuration");

        let primary_rpc_server_socket =
            wcn_rpc::server::Socket::new_high_priority(raw.primary_rpc_server_port)
                .context("Failed to bind primary rpc server socket")?;

        let secondary_rpc_server_socket =
            wcn_rpc::server::Socket::new_low_priority(raw.secondary_rpc_server_port)
                .context("Failed to bind secondary rpc server socket")?;

        let metrics_server_socket = TcpListener::bind(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            raw.metrics_server_port,
        ))
        .context("Failed to bind metrics server socket")?;

        Ok(Self {
            keypair: raw.keypair,
            primary_rpc_server_socket,
            secondary_rpc_server_socket,
            metrics_server_socket,
            connection_timeout: raw
                .db_connection_timeout_ms
                .unwrap_or(10_000)
                .pipe(Duration::from_millis),
            max_connections: raw.db_max_connections.unwrap_or(500),
            max_connections_per_ip: raw.db_max_connections_per_ip.unwrap_or(50),
            max_connection_rate_per_ip: raw.db_max_connection_rate_per_ip.unwrap_or(50),
            max_concurrent_rpcs: raw.db_max_concurrent_rpcs.unwrap_or(4000),
            rocksdb_dir: raw.rocksdb_dir,
            rocksdb,
            shutdown_signal: ShutdownSignal::new(),
            prometheus_handle,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(deserialize_with = "deserialize_keypair")]
    #[serde(rename = "secret_key")]
    keypair: Keypair,

    primary_rpc_server_port: u16,
    secondary_rpc_server_port: u16,
    metrics_server_port: u16,

    db_connection_timeout_ms: Option<u64>,
    db_max_connections: Option<u32>,
    db_max_connections_per_ip: Option<u32>,
    db_max_connection_rate_per_ip: Option<u32>,
    db_max_concurrent_rpcs: Option<u32>,

    rocksdb_dir: PathBuf,
    rocksdb_num_batch_threads: Option<usize>,
    rocksdb_num_callback_threads: Option<usize>,
    rocksdb_max_subcompactions: Option<usize>,
    rocksdb_max_background_jobs: Option<usize>,
    rocksdb_ratelimiter: Option<usize>,
    rocksdb_increase_parallelism: Option<usize>,
    rocksdb_write_buffer_size: Option<usize>,
    rocksdb_max_write_buffer_number: Option<usize>,
    rocksdb_min_write_buffer_number_to_merge: Option<usize>,
    rocksdb_block_cache_size: Option<usize>,
    rocksdb_block_size: Option<usize>,
    rocksdb_row_cache_size: Option<usize>,
    rocksdb_enable_metrics: Option<bool>,
}

fn deserialize_keypair<'de, D>(deserializer: D) -> Result<Keypair, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error as _;
    String::deserialize(deserializer)
        .and_then(|s| {
            base64::engine::general_purpose::STANDARD
                .decode(s)
                .map_err(D::Error::custom)
        })
        .and_then(|bytes| Keypair::ed25519_from_bytes(bytes).map_err(D::Error::custom))
}

fn create_rocksdb_config(raw: &RawConfig) -> RocksdbDatabaseConfig {
    let defaults = RocksdbDatabaseConfig::default();

    RocksdbDatabaseConfig {
        num_batch_threads: raw
            .rocksdb_num_batch_threads
            .unwrap_or(defaults.num_batch_threads),
        num_callback_threads: raw
            .rocksdb_num_callback_threads
            .unwrap_or(defaults.num_callback_threads),
        max_subcompactions: raw
            .rocksdb_max_subcompactions
            .unwrap_or(defaults.max_subcompactions),
        max_background_jobs: raw
            .rocksdb_max_background_jobs
            .unwrap_or(defaults.max_background_jobs),

        ratelimiter: raw
            .rocksdb_ratelimiter
            .tap_none(|| {
                tracing::warn!(
                    default = defaults.ratelimiter,
                    "rocksdb `ratelimiter` param not set, using default value"
                );
            })
            .unwrap_or(defaults.ratelimiter),

        increase_parallelism: raw
            .rocksdb_increase_parallelism
            .unwrap_or(defaults.increase_parallelism),

        write_buffer_size: raw
            .rocksdb_write_buffer_size
            .tap_none(|| {
                tracing::warn!(
                    default = defaults.write_buffer_size,
                    "rocksdb `write_buffer_size` param not set, using default value"
                );
            })
            .unwrap_or(defaults.write_buffer_size),

        max_write_buffer_number: raw
            .rocksdb_max_write_buffer_number
            .tap_none(|| {
                tracing::warn!(
                    default = defaults.max_write_buffer_number,
                    "rocksdb `max_write_buffer_number` param not set, using default value"
                );
            })
            .unwrap_or(defaults.max_write_buffer_number),

        min_write_buffer_number_to_merge: raw
            .rocksdb_min_write_buffer_number_to_merge
            .unwrap_or(defaults.min_write_buffer_number_to_merge),

        block_cache_size: raw
            .rocksdb_block_cache_size
            .tap_none(|| {
                tracing::warn!(
                    default = defaults.block_cache_size,
                    "rocksdb `block_cache_size` param not set, using default value"
                );
            })
            .unwrap_or(defaults.block_cache_size),

        block_size: raw.rocksdb_block_size.unwrap_or(defaults.block_size),

        row_cache_size: raw
            .rocksdb_row_cache_size
            .tap_none(|| {
                tracing::warn!(
                    default = defaults.row_cache_size,
                    "rocksdb `row_cache_size` param not set, using default value"
                );
            })
            .unwrap_or(defaults.row_cache_size),

        enable_metrics: raw
            .rocksdb_enable_metrics
            .unwrap_or(defaults.enable_metrics),
    }
}
