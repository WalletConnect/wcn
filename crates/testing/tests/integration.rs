use {
    std::{
        net::{Ipv4Addr, SocketAddrV4},
        time::Duration,
    },
    tracing_subscriber::EnvFilter,
    wcn_client::PeerAddr,
    wcn_testing::TestCluster,
};

#[tokio::test]
async fn test_suite() {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .with_env_filter(EnvFilter::from_default_env())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let mut cluster = TestCluster::deploy().await;

    tokio::time::sleep(Duration::from_secs(5)).await;
    test_client_encryption(&cluster).await;

    cluster
        .under_load(async |cluster| {
            tokio::time::sleep(Duration::from_secs(5)).await;

            cluster.shutdown_one_node_per_node_operator().await;

            cluster.redeploy_all_offline_nodes().await;

            cluster
                .replace_all_node_operators_except_namespace_owner()
                .await;
        })
        .await;
}

async fn test_client_encryption(cluster: &TestCluster) {
    let client = cluster.authorized_client();
    let ns = client.authorized_namespace();
    let operator = cluster.node_operator(client.operator_id()).unwrap();

    let create_client = || {
        wcn_client::Builder::new(wcn_client::Config {
            keypair: client.keypair().clone(),
            cluster_key: wcn_cluster::testing::encryption_key(),
            connection_timeout: Duration::from_secs(1),
            max_idle_connection_timeout: Duration::from_secs(5),
            operation_timeout: Duration::from_secs(2),
            reconnect_interval: Duration::from_millis(100),
            max_concurrent_rpcs: 5000,
            nodes: operator
                .nodes()
                .iter()
                .map(|node| PeerAddr {
                    id: node.peer_id(),
                    addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, node.primary_rpc_server_port()),
                })
                .collect(),
            max_retries: 3,
        })
    };

    let client1 = create_client()
        .with_encryption(wcn_client::EncryptionKey::new(b"12345").unwrap())
        .build()
        .await
        .unwrap();

    let client2 = create_client()
        .with_encryption(wcn_client::EncryptionKey::new(b"23456").unwrap())
        .build()
        .await
        .unwrap();

    let unencrypted_client = create_client().build().await.unwrap();

    let ttl = Duration::from_secs(60);

    unencrypted_client
        .set(ns, b"unencrypted_string_key", b"data", ttl)
        .await
        .unwrap();

    // Fails to decrypt unencrypted data.
    let err = client1
        .get(ns, b"unencrypted_string_key")
        .await
        .err()
        .unwrap();
    assert!(matches!(err, wcn_client::Error::Encryption(_)));

    client1
        .set(ns, b"encrypted_string_key", b"data", ttl)
        .await
        .unwrap();

    // Successfully decrypts data.
    let rec = client1
        .get(ns, b"encrypted_string_key")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.value, b"data");

    // Fails to decrypt with different key.
    let err = client2
        .get(ns, b"encrypted_string_key")
        .await
        .err()
        .unwrap();
    assert!(matches!(err, wcn_client::Error::Encryption(_)));

    // Encryption for hset/hget.
    client1
        .hset(ns, b"encrypted_map_key", b"field", b"data", ttl)
        .await
        .unwrap();

    // Successfully decrypts data.
    let rec = client1
        .hget(ns, b"encrypted_map_key", b"field")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.value, b"data");

    // Fails to decrypt with different key.
    let err = client2
        .hget(ns, b"encrypted_map_key", b"field")
        .await
        .err()
        .unwrap();
    assert!(matches!(err, wcn_client::Error::Encryption(_)));

    tracing::info!("WCN client encryption tests passed");
}
