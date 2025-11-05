use std::time::Duration;

use libp2p_identity::ed25519::Keypair;
use wcn_rpc::{server::{self, Server, ShutdownSignal, Socket}, transport::Priority};
use wcn_storage_api::{Operation, Result, StorageApi, operation::Output, rpc};

#[derive(Clone)]
struct TestServer;

impl StorageApi for TestServer {
    fn execute(
        &self,
        _operation: Operation<'_>,
    ) -> impl Future<Output = Result<Output>> + Send {
        async { Ok(Output::none()) }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _logger = wcn_logging::Logger::init(wcn_logging::LogFormat::Json, Some("INFO"), None);
    let socket = Socket::new(7474, Priority::High)?;
    let keypair = Keypair::try_from_bytes(&mut include_bytes!("../keypair").to_vec())?.into();
    let connection_timeout = Duration::from_secs(10);
    let max_connections = 1000;
    let shutdown_signal = ShutdownSignal::new();
    let cfg = wcn_rpc::server::Config {
        name: "Test Server",
        socket,
        keypair,
        connection_timeout,
        max_connections,
        max_connections_per_ip: max_connections,
        max_connection_rate_per_ip: max_connections,
        max_concurrent_rpcs: max_connections,
        max_idle_connection_timeout: connection_timeout,
        shutdown_signal,
    };
    let api = rpc::DatabaseApi::new();
    let api = api.with_state(TestServer);
    server::new(api).serve(cfg)?.await;
    Ok(())
}
