use std::{net::{Ipv4Addr, SocketAddrV4}, str::FromStr, time::Duration};

use anyhow::bail;
use libp2p_identity::{Keypair, ed25519};
use tokio::task::{self, JoinSet};
use tracing::info;
use wcn_rpc::{Client, PeerId, client::{Api, Config}, transport::Priority};
use wcn_storage_api::{Namespace, RecordVersion, StorageApi, operation::Del, rpc::DatabaseApi};

const NUM_CONCURRENT: u32 = 1_000;
const NUM_MESSAGES: u32 = 1_000;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _logger = wcn_logging::Logger::init(wcn_logging::LogFormat::Json, Some("INFO"), None);
    let keypair = Keypair::generate_ed25519();
    let connection_timeout = Duration::from_secs(1000);
    let reconnect_interval = Duration::from_secs(1);
    let max_concurrent_rpcs = 1000;
    let priority = Priority::High;
    let config = Config {
        keypair,
        connection_timeout,
        reconnect_interval,
        max_idle_connection_timeout: connection_timeout,
        max_concurrent_rpcs,
        priority,
    };
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7474);
    let server_keypair = ed25519::Keypair::try_from_bytes(&mut include_bytes!("../../quinn_test_server/keypair").to_vec())?;
    let peer_id = PeerId::from_public_key(&server_keypair.public().into());
    let client = wcn_storage_api::rpc::DatabaseApi::new().try_into_client(config)?;
    let namespace = Namespace::from_str("6b6977696b6977696b6977696b6977696b697769/00")?;

    let mut join_set = JoinSet::new();
    for _id in 0..NUM_CONCURRENT {
        join_set.spawn(tokio::spawn(run(client.clone(), addr, peer_id, namespace)));
    }
    let results = join_set.join_all().await;
    let mut total_successes = 0;
    let mut total_failures = 0;
    for successes_failures in results {
        let (successes, failures) = successes_failures??;
        total_successes += successes;
        total_failures += failures;
    }
    info!("total successes:{} failure:{} NUM_CONCURRENT:{} NUM_MESSAGES:{}", total_successes, total_failures, NUM_CONCURRENT, NUM_MESSAGES);

    Ok(())
}

async fn run(client: Client<DatabaseApi>, addr: SocketAddrV4, peer_id: PeerId, namespace: Namespace) -> anyhow::Result<(u32, u32)> {
    info!("job {:?} started", task::id());
    let mut successes = 0;
    let mut failures = 0;
    let mut retries = 10;
    while successes + failures < NUM_MESSAGES {
        let Ok(connection) = client.connect(addr, &peer_id, ()).await else {
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
        for _m in 0..NUM_MESSAGES {
            match connection.execute(del.clone().into()).await {
                Ok(_) => successes += 1,
                Err(_) => failures += 1,
            }
        }
    }
    info!("job {:?} finished, successes:{} failures:{}", task::id(), successes, failures);
    Ok((successes, failures))
}
