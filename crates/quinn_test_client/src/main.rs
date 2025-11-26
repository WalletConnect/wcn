use {
    anyhow::bail,
    clap::Parser,
    libp2p_identity::{Keypair, ed25519},
    quinn_proto::{ConnectionStats, FrameStats, PathStats, UdpStats},
    std::{net::SocketAddrV4, str::FromStr, time::Duration},
    tokio::task::{self, JoinSet},
    tracing::{info, warn},
    wcn_rpc::{Client, PeerId, client, transport::Priority},
    wcn_storage_api::{Namespace, RecordVersion, StorageApi, operation::Del, rpc::DatabaseApi},
};

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
    let _logger = wcn_logging::Logger::init(wcn_logging::LogFormat::Text, Some("INFO"), None);
    let keypair = Keypair::generate_ed25519();
    let client_config = client::Config {
        keypair,
        connection_timeout: Duration::from_secs(config.connection_timeout_secs),
        reconnect_interval: Duration::from_secs(config.reconnect_interval_secs),
        max_idle_connection_timeout: Duration::from_secs(config.max_idle_connection_timeout_secs),
        max_concurrent_rpcs: config.max_concurrent_rpcs,
        priority: Priority::High,
    };
    let server_keypair = ed25519::Keypair::try_from_bytes(
        &mut include_bytes!("../../operator_cli/keypair").to_vec(),
    )?;
    let peer_id = PeerId::from_public_key(&server_keypair.public().into());
    let client = client::Client::new(client_config, wcn_storage_api::rpc::DatabaseApi::new())?;
    let namespace = Namespace::from_str("6b6977696b6977696b6977696b6977696b697769/00")?;

    let mut join_set = JoinSet::new();
    for _id in 0..config.num_concurrent_clients {
        join_set.spawn(tokio::spawn(run(
            client.clone(),
            config.clone(),
            peer_id,
            namespace,
        )));
    }
    let results = join_set.join_all().await;
    let mut total_successes = 0;
    let mut total_failures = 0;
    let mut total_connection_stats = ConnectionStats::default();
    for successes_failures in results {
        let (successes, failures, connection_stats) = successes_failures??;
        total_successes += successes;
        total_failures += failures;
        merge_connection_stats(&mut total_connection_stats, connection_stats);
    }
    println!("{:#?}", total_connection_stats);
    info!(
        "total successes:{} failures:{} num_concurrent_clients:{} num_messages_per_client:{}",
        total_successes,
        total_failures,
        config.num_concurrent_clients,
        config.num_messages_per_client
    );

    Ok(())
}

async fn run(
    client: Client<DatabaseApi>,
    config: Config,
    peer_id: PeerId,
    namespace: Namespace,
) -> anyhow::Result<(u32, u32, ConnectionStats)> {
    info!("job {:?} started", task::id());
    let mut successes = 0;
    let mut failures = 0;
    let mut retries = 10;
    let mut connection_stats = ConnectionStats::default();
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
            key: vec![1u8; 8],
            version: RecordVersion::from_unix_timestamp_micros(0),
            keyspace_version: None,
        };
        while successes + failures < config.num_messages_per_client {
            match connection.execute(del.clone().into()).await {
                Ok(_) => successes += 1,
                Err(_) => failures += 1,
            }
        }
        match connection.quinn_stats() {
            Some(stats) => merge_connection_stats(&mut connection_stats, stats),
            None => warn!(
                "job {:?} no connection stats found for current connection",
                task::id()
            ),
        }
    }
    info!(
        "job {:?} finished, successes:{} failures:{}",
        task::id(),
        successes,
        failures
    );
    Ok((successes, failures, connection_stats))
}

fn merge_connection_stats(stats1: &mut ConnectionStats, stats2: ConnectionStats) {
    merge_frame_stats(&mut stats1.frame_rx, stats2.frame_rx);
    merge_frame_stats(&mut stats1.frame_tx, stats2.frame_tx);
    merge_path_stats(&mut stats1.path, stats2.path);
    merge_udp_stats(&mut stats1.udp_rx, stats2.udp_rx);
    merge_udp_stats(&mut stats1.udp_tx, stats2.udp_tx);
}

fn merge_frame_stats(stats1: &mut FrameStats, stats2: FrameStats) {
    stats1.ack_frequency = stats1.ack_frequency.saturating_add(stats2.ack_frequency);
    stats1.acks = stats1.acks.saturating_add(stats2.acks);
    stats1.connection_close = stats1
        .connection_close
        .saturating_add(stats2.connection_close);
    stats1.crypto = stats1.crypto.saturating_add(stats2.crypto);
    stats1.data_blocked = stats1.data_blocked.saturating_add(stats2.data_blocked);
    stats1.datagram = stats1.datagram.saturating_add(stats2.datagram);
    stats1.handshake_done = stats1.handshake_done.saturating_add(stats2.handshake_done);
    stats1.immediate_ack = stats1.immediate_ack.saturating_add(stats2.immediate_ack);
    stats1.max_data = stats1.max_data.saturating_add(stats2.max_data);
    stats1.max_stream_data = stats1
        .max_stream_data
        .saturating_add(stats2.max_stream_data);
    stats1.max_streams_bidi = stats1
        .max_streams_bidi
        .saturating_add(stats2.max_streams_bidi);
    stats1.max_streams_uni = stats1
        .max_streams_uni
        .saturating_add(stats2.max_streams_uni);
    stats1.new_connection_id = stats1
        .new_connection_id
        .saturating_add(stats2.new_connection_id);
    stats1.new_token = stats1.new_token.saturating_add(stats2.new_token);
    stats1.path_challenge = stats1.path_challenge.saturating_add(stats2.path_challenge);
    stats1.path_response = stats1.path_response.saturating_add(stats2.path_response);
    stats1.ping = stats1.ping.saturating_add(stats2.ping);
    stats1.reset_stream = stats1.reset_stream.saturating_add(stats2.reset_stream);
    stats1.retire_connection_id = stats1
        .retire_connection_id
        .saturating_add(stats2.retire_connection_id);
    stats1.stop_sending = stats1.stop_sending.saturating_add(stats2.stop_sending);
    stats1.stream = stats1.stream.saturating_add(stats2.stream);
    stats1.stream_data_blocked = stats1
        .stream_data_blocked
        .saturating_add(stats2.stream_data_blocked);
    stats1.streams_blocked_bidi = stats1
        .streams_blocked_bidi
        .saturating_add(stats2.streams_blocked_bidi);
    stats1.streams_blocked_uni = stats1
        .streams_blocked_uni
        .saturating_add(stats2.streams_blocked_uni);
}

fn merge_path_stats(stats1: &mut PathStats, stats2: PathStats) {
    stats1.black_holes_detected = stats1
        .black_holes_detected
        .saturating_add(stats2.black_holes_detected);
    stats1.congestion_events = stats1
        .congestion_events
        .saturating_add(stats2.congestion_events);
    stats1.current_mtu = stats1.current_mtu.saturating_add(stats2.current_mtu);
    stats1.cwnd = stats1.cwnd.saturating_add(stats2.cwnd);
    stats1.lost_bytes = stats1.lost_bytes.saturating_add(stats2.lost_bytes);
    stats1.lost_packets = stats1.lost_packets.saturating_add(stats2.lost_packets);
    stats1.lost_plpmtud_probes = stats1
        .lost_plpmtud_probes
        .saturating_add(stats2.lost_plpmtud_probes);
    stats1.rtt = stats1.rtt.saturating_add(stats2.rtt);
    stats1.sent_packets = stats1.sent_packets.saturating_add(stats2.sent_packets);
    stats1.sent_plpmtud_probes = stats1
        .sent_plpmtud_probes
        .saturating_add(stats2.sent_plpmtud_probes);
}

fn merge_udp_stats(stats1: &mut UdpStats, stats2: UdpStats) {
    stats1.bytes = stats1.bytes.saturating_add(stats2.bytes);
    stats1.datagrams = stats1.datagrams.saturating_add(stats2.datagrams);
    stats1.ios = stats1.ios.saturating_add(stats2.ios);
}
