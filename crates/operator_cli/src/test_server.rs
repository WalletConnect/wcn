use {
    clap::Args,
    libp2p_identity::ed25519::Keypair,
    std::time::Duration,
    wcn_rpc::{
        server::{self, Server, ShutdownSignal, Socket},
        transport::Priority,
    },
    wcn_storage_api::{Operation, Result, StorageApi, operation::Output, rpc},
};

#[derive(Debug, Args)]
pub struct Command {
    #[arg(short, long)]
    port: u16,

    #[arg(short, long, default_value_t = 2_000)]
    connection_timeout_ms: u64,

    /// Maximum number of connections accepted in total
    #[arg(short, long, default_value_t = 2000)]
    max_connections: u32,

    /// Maximum number of connection accepted per ip
    #[arg(short = 'M', long, default_value_t = 1000)]
    max_connections_per_ip: u32,

    /// Maximum connection rate accepted per ip
    #[arg(short = 'I', long, default_value_t = 1000)]
    max_connection_rate_per_ip: u32,

    /// Maximum number of concurrent rpcs
    #[arg(short = 'r', long, default_value_t = 10_000)]
    max_concurrent_rpcs: u32,

    #[arg(long, default_value_t = 500)]
    max_idle_connection_timeout_ms: u64,
}

#[derive(Clone)]
struct TestServer;

impl StorageApi for TestServer {
    async fn execute(&self, _operation: Operation<'_>) -> Result<Output> {
        Ok(Output::none())
    }
}

pub(super) async fn execute(args: Command) -> anyhow::Result<()> {
    let _logger = wcn_logging::Logger::init(wcn_logging::LogFormat::Text, Some("INFO"), None);
    let socket = Socket::new(args.port, Priority::High)?;
    let keypair = Keypair::try_from_bytes(&mut include_bytes!("../keypair").to_vec())?.into();
    let shutdown_signal = ShutdownSignal::new();
    let cfg = wcn_rpc::server::Config {
        name: "Test Server",
        socket,
        keypair,
        connection_timeout: Duration::from_millis(args.connection_timeout_ms),
        max_connections: args.max_connections,
        max_connections_per_ip: args.max_connections_per_ip,
        max_connection_rate_per_ip: args.max_connection_rate_per_ip,
        max_concurrent_rpcs: args.max_concurrent_rpcs,
        max_idle_connection_timeout: Duration::from_millis(args.max_idle_connection_timeout_ms),
        shutdown_signal,
    };
    let api = rpc::DatabaseApi::new();
    let api = api.with_state(TestServer);
    server::new(api).serve(cfg)?.await;
    Ok(())
}
