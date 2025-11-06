use {
    clap::Parser,
    libp2p_identity::ed25519::Keypair,
    std::time::Duration,
    wcn_rpc::{
        server::{self, Server, ShutdownSignal, Socket},
        transport::Priority,
    },
    wcn_storage_api::{Operation, Result, StorageApi, operation::Output, rpc},
};

#[derive(Parser)]
struct Config {
    #[arg(short = 'p', long)]
    port: u16,

    #[arg(short = 'c', long, default_value_t = 10)]
    connection_timeout_secs: u64,

    #[arg(short = 'm', long, default_value_t = 100)]
    max_connections: u32,

    #[arg(short = 'M', long, default_value_t = 100)]
    max_connections_per_ip: u32,

    #[arg(short = 'I', long, default_value_t = 100)]
    max_connection_rate_per_ip: u32,

    #[arg(short = 'r', long, default_value_t = 1000)]
    max_concurrent_rpcs: u32,

    #[arg(short = 'i', long, default_value_t = 10)]
    max_idle_connection_timeout_secs: u64,
}

#[derive(Clone)]
struct TestServer;

impl StorageApi for TestServer {
    fn execute(&self, _operation: Operation<'_>) -> impl Future<Output = Result<Output>> + Send {
        async { Ok(Output::none()) }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    let _logger = wcn_logging::Logger::init(wcn_logging::LogFormat::Json, Some("INFO"), None);
    let socket = Socket::new(config.port, Priority::High)?;
    let keypair = Keypair::try_from_bytes(&mut include_bytes!("../keypair").to_vec())?.into();
    let shutdown_signal = ShutdownSignal::new();
    let cfg = wcn_rpc::server::Config {
        name: "Test Server",
        socket,
        keypair,
        connection_timeout: Duration::from_secs(config.connection_timeout_secs),
        max_connections: config.max_connections,
        max_connections_per_ip: config.max_connections_per_ip,
        max_connection_rate_per_ip: config.max_connection_rate_per_ip,
        max_concurrent_rpcs: config.max_concurrent_rpcs,
        max_idle_connection_timeout: Duration::from_secs(config.max_idle_connection_timeout_secs),
        shutdown_signal,
    };
    let api = rpc::DatabaseApi::new();
    let api = api.with_state(TestServer);
    server::new(api).serve(cfg)?.await;
    Ok(())
}
