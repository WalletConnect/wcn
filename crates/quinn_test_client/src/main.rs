use std::{net::SocketAddrV4, str::FromStr, time::Duration};

use anyhow::bail;
use clap::Parser;
use libp2p_identity::{Keypair, ed25519};
use tokio::task::{self, JoinSet};
use tracing::info;
use wcn_rpc::{Client, PeerId, client, transport::Priority};
use wcn_storage_api::{Namespace, RecordVersion, StorageApi, operation::Del, rpc::DatabaseApi};

#[derive(Clone, Parser)]
struct Config {
    #[arg(short = 'c', long, default_value_t = 10)]
    connection_timeout_secs: u64,

    #[arg(short = 'M', long, default_value_t = 1000)]
    max_concurrent_rpcs: u32,

    #[arg(short = 'i', long, default_value_t = 10)]
    max_idle_connection_timeout_secs: u64,

    #[arg(short = 'n', long, default_value_t = 100)]
    num_concurrent_clients: u32,

    #[arg(short = 'm', long, default_value_t = 1_000)]
    num_messages_per_client: u32,

    #[arg(short = 'r', long, default_value_t = 1)]
    reconnect_interval_secs: u64,

    #[arg(short = 's', long)]
    server_address: SocketAddrV4,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    let _logger = wcn_logging::Logger::init(wcn_logging::LogFormat::Json, Some("INFO"), None);
    let keypair = Keypair::generate_ed25519();
    let client_config = client::Config {
        keypair,
        connection_timeout: Duration::from_secs(config.connection_timeout_secs),
        reconnect_interval: Duration::from_secs(config.reconnect_interval_secs),
        max_idle_connection_timeout: Duration::from_secs(config.max_idle_connection_timeout_secs),
        max_concurrent_rpcs: config.max_concurrent_rpcs,
        priority: Priority::High,
    };
    let server_keypair = ed25519::Keypair::try_from_bytes(&mut include_bytes!("../../quinn_test_server/keypair").to_vec())?;
    let peer_id = PeerId::from_public_key(&server_keypair.public().into());
    let client = client::Client::new(client_config, wcn_storage_api::rpc::DatabaseApi::new())?;
    let namespace = Namespace::from_str("6b6977696b6977696b6977696b6977696b697769/00")?;

    let mut join_set = JoinSet::new();
    for _id in 0..config.num_concurrent_clients {
        join_set.spawn(tokio::spawn(run(client.clone(), config.clone(), peer_id, namespace)));
    }
    let results = join_set.join_all().await;
    let mut total_successes = 0;
    let mut total_failures = 0;
    for successes_failures in results {
        let (successes, failures) = successes_failures??;
        total_successes += successes;
        total_failures += failures;
    }
    info!("total successes:{} failures:{} num_concurrent_clients:{} num_messages_per_client:{}", total_successes, total_failures, config.num_concurrent_clients, config.num_messages_per_client);

    Ok(())
}

async fn run(client: Client<DatabaseApi>, config: Config, peer_id: PeerId, namespace: Namespace) -> anyhow::Result<(u32, u32)> {
    info!("job {:?} started", task::id());
    let mut successes = 0;
    let mut failures = 0;
    let mut retries = 10;
    while successes + failures < config.num_messages_per_client {
        let Ok(connection) = client.connect(config.server_address, &peer_id, ()).await else {
            retries -= 1;
            if retries == 0 {
                bail!("Unable to connect to the server");
            }
            tracing::warn!("Unable to connect, retrying");
            continue;
        };
        let del = Del {
            namespace,
            key: vec![1u8;8],
            version: RecordVersion::from_unix_timestamp_micros(0),
            keyspace_version: None,
        };
        while successes + failures < config.num_messages_per_client {
            match connection.execute(del.clone().into()).await {
                Ok(_) => successes += 1,
                Err(_) => failures += 1,
            }
        }
    }
    info!("job {:?} finished, successes:{} failures:{}", task::id(), successes, failures);
    Ok((successes, failures))
}
