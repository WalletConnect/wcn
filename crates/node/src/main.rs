use {
    anyhow::Context,
    base64::Engine as _,
    libp2p_identity::Keypair,
    serde::Deserialize,
    std::{
        net::{Ipv4Addr, SocketAddrV4, TcpListener},
        time::Duration,
    },
    tap::Tap,
    wc::metrics::exporter_prometheus::{PrometheusBuilder, PrometheusHandle},
    wcn_cluster::smart_contract,
    wcn_node::Config,
    wcn_rpc::server::{run_with_signal_handling, ShutdownSignal},
};

#[global_allocator]
static GLOBAL: wc::alloc::Jemalloc = wc::alloc::Jemalloc;

// Each field name in this struct corresponds to the evironment variable
// (upper-cased).
#[derive(Debug, Deserialize)]
struct EnvConfig {
    secret_key: String,

    primary_rpc_server_port: u16,
    secondary_rpc_server_port: u16,
    metrics_server_port: u16,

    max_idle_connection_timeout_ms: Option<u32>,

    database_rpc_server_address: String,
    database_peer_id: String,
    database_primary_rpc_server_port: u16,
    database_secondary_rpc_server_port: u16,

    smart_contract_address: String,
    smart_contract_signer_private_key: Option<String>,
    smart_contract_encryption_key: String,
    rpc_provider_url: String,

    public_address: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let _logger = wcn_logging::Logger::init(wcn_logging::LogFormat::Json, None, None);

    let prometheus_handle = PrometheusBuilder::new()
        .install_recorder()
        .context("install Prometheus recorder")?;

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
    wc::metrics::gauge!("wcn_node_version").set(version);

    let env: EnvConfig = envy::from_env()?;

    let cfg = new_config(&env, prometheus_handle).context("failed to parse config")?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let shutdown_signal = cfg.shutdown_signal.clone();
            let run_fut = wcn_node::run(cfg)
                .await?
                .tap(|_| tracing::info!("node stopped"));
            let r = run_with_signal_handling::<_, anyhow::Error>(shutdown_signal, run_fut).await?;
            Ok(r)
        })
}

fn new_config(env: &EnvConfig, prometheus_handle: PrometheusHandle) -> anyhow::Result<Config> {
    let secret_key = base64::engine::general_purpose::STANDARD
        .decode(&env.secret_key)
        .context("SECRET_KEY")?;

    let keypair = Keypair::ed25519_from_bytes(secret_key).context("SECRET_KEY")?;

    let primary_rpc_server_socket =
        wcn_rpc::server::Socket::new_high_priority(env.primary_rpc_server_port)
            .context("Failed to bind primary rpc server socket")?;

    let secondary_rpc_server_socket =
        wcn_rpc::server::Socket::new_low_priority(env.secondary_rpc_server_port)
            .context("Failed to bind secondary rpc server socket")?;

    let metrics_server_socket = TcpListener::bind(SocketAddrV4::new(
        Ipv4Addr::UNSPECIFIED,
        env.metrics_server_port,
    ))
    .context("Failed to bind metrics server socket")?;

    let max_idle_connection_timeout =
        Duration::from_millis(env.max_idle_connection_timeout_ms.unwrap_or(500) as u64);

    let smart_contract_address = env
        .smart_contract_address
        .parse()
        .context("SMART_CONTRACT_ADDRESS")?;

    let smart_contract_signer = env
        .smart_contract_signer_private_key
        .as_ref()
        .map(|key| smart_contract::evm::Signer::try_from_private_key(key))
        .transpose()
        .context("SMART_CONTRACT_SIGNER_PRIVATE_KEY")?;

    let smart_contract_encryption_key = env
        .smart_contract_encryption_key
        .parse()
        .context("SMART_CONTRACT_ENCRYPTION_KEY")?;

    let rpc_provider_url = env.rpc_provider_url.parse().context("RPC_PROVIDER_URL")?;

    let database_rpc_server_address = env
        .database_rpc_server_address
        .parse()
        .context("DATABASE_RPC_SERVER_ADDRESS")?;

    let database_peer_id = env.database_peer_id.parse().context("DATABASE_PEER_ID")?;

    let public_address = env
        .public_address
        .as_ref()
        .map(|a| a.parse().context("PUBLIC_ADDRESS"))
        .transpose()?;

    Ok(Config {
        keypair,
        primary_rpc_server_socket,
        secondary_rpc_server_socket,
        metrics_server_socket,
        max_idle_connection_timeout,
        smart_contract_address,
        smart_contract_signer,
        smart_contract_encryption_key,
        rpc_provider_url,
        database_rpc_server_address,
        database_peer_id,
        database_primary_rpc_server_port: env.database_primary_rpc_server_port,
        database_secondary_rpc_server_port: env.database_secondary_rpc_server_port,
        shutdown_signal: ShutdownSignal::new(),
        prometheus_handle,
        public_address,
    })
}
