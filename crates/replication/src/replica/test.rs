use {
    crate::{replica, Replica},
    derive_more::derive::AsRef,
    futures::TryStreamExt as _,
    std::{sync::Arc, time::Duration},
    wcn_cluster::{
        keyspace,
        migration,
        node_operator,
        smart_contract::{self, testing::FakeSmartContract},
        testing::node_peer_id,
        Cluster,
        EncryptionKey,
        Node,
        PeerId,
    },
    wcn_storage_api::{
        operation,
        testing::FakeStorage,
        Error,
        Factory,
        Namespace,
        Record,
        RecordVersion,
        StorageApi,
    },
};

#[derive(AsRef, Clone)]
struct Config {
    #[as_ref]
    encryption_key: EncryptionKey,
    smart_contract_registry: smart_contract::testing::FakeRegistry,
    database: FakeStorage,
}

impl wcn_cluster::Config for Config {
    type SmartContract = FakeSmartContract;
    type KeyspaceShards = ();
    type Node = Node;

    fn new_node(&self, _operator_id: node_operator::Id, node: Node) -> Node {
        node
    }
}

impl super::Config for Config {
    type OutboundDatabaseConnection = FakeStorage;
}

struct Context {
    config: Config,

    replica: Replica<Config>,
    conn: replica::InboundConnection<Config>,
}

impl Context {
    async fn new() -> Self {
        let cfg = Config {
            encryption_key: wcn_cluster::testing::encryption_key(),
            smart_contract_registry: Default::default(),
            database: Default::default(),
        };

        let cluster = Cluster::deploy(
            cfg.clone(),
            &cfg.smart_contract_registry
                .deployer(smart_contract::testing::account_address(42)),
            wcn_cluster::Settings {
                max_node_operator_data_bytes: 1024,
                event_propagation_latency: Duration::from_secs(4),
                clock_skew: Duration::from_secs(2),
            },
            (0..8)
                .map(|idx| wcn_cluster::testing::node_operator(idx as u8))
                .collect(),
        )
        .await
        .unwrap();

        let replica = Replica::new(Arc::new(cfg.clone()), cluster, cfg.database.clone());

        Self {
            config: cfg,
            conn: replica.new_inbound_connection(node_peer_id(0, 0)).unwrap(),
            replica,
        }
    }

    async fn start_migration(&self) {
        let new_operator = wcn_cluster::testing::node_operator(100_u8);

        self.replica
            .cluster
            .add_node_operator(new_operator.clone())
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        let plan = migration::Plan {
            remove: [].into(),
            add: [new_operator.id].into(),
            replication_strategy: keyspace::ReplicationStrategy::UniformDistribution,
        };

        self.replica.cluster.start_migration(plan).await.unwrap();
        self.wait_migration().await;
    }

    async fn wait_migration(&self) {
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if self.replica.cluster.view().migration().is_some() {
                    return;
                }

                tokio::time::sleep(Duration::from_millis(100)).await
            }
        })
        .await
        .unwrap()
    }
}

fn namespace(operator_id: u8, idx: u8) -> Namespace {
    let operator_id = wcn_cluster::testing::node_operator(operator_id).id;
    format!("{operator_id}/{idx}").parse().unwrap()
}

#[tokio::test]
async fn inbound_connections_not_authorized_for_nodes_not_in_cluster() {
    let ctx = Context::new().await;
    let err = ctx.replica.new_storage_api(PeerId::random()).err().unwrap();

    assert_eq!(err, Error::unauthorized())
}

#[tokio::test]
async fn errors_on_keyspace_version_mismatch() {
    let ctx = Context::new().await;

    let get = operation::GetBorrowed {
        namespace: namespace(0, 0),
        key: b"test",
        keyspace_version: None,
    };

    let res = ctx.conn.execute(operation::Borrowed::Get(get).into()).await;
    assert_eq!(res, Err(Error::keyspace_version_mismatch()));

    let get = operation::GetBorrowed {
        namespace: namespace(0, 0),
        key: b"test",
        keyspace_version: Some(1),
    };

    let res = ctx.conn.execute(operation::Borrowed::Get(get).into()).await;
    assert_eq!(res, Err(Error::keyspace_version_mismatch()));

    let err = ctx.conn.read_data(0..=42, 1).await.err();
    assert_eq!(err, Some(Error::keyspace_version_mismatch()));
}

#[tokio::test]
async fn accepts_only_prev_keyspace_version_before_time_skew_window() {
    let ctx = Context::new().await;
    ctx.start_migration().await;

    let get = operation::GetBorrowed {
        namespace: namespace(0, 0),
        key: b"foo",
        keyspace_version: Some(0),
    };

    let res = ctx.conn.execute(operation::Borrowed::Get(get).into()).await;
    assert_eq!(res, Ok(operation::Output::Record(None)));

    let get = operation::GetBorrowed {
        namespace: namespace(0, 0),
        key: b"foo",
        keyspace_version: Some(1),
    };

    let res = ctx.conn.execute(operation::Borrowed::Get(get).into()).await;
    assert_eq!(res, Err(Error::keyspace_version_mismatch()));
}

#[tokio::test]
async fn accepts_both_keyspace_versions_during_time_skew_window() {
    let ctx = Context::new().await;
    ctx.start_migration().await;

    let settings = *ctx.replica.cluster.view().settings();

    tokio::time::sleep(settings.event_propagation_latency).await;

    let get = operation::GetBorrowed {
        namespace: namespace(0, 0),
        key: b"foo",
        keyspace_version: Some(0),
    };

    let res = ctx.conn.execute(operation::Borrowed::Get(get).into()).await;
    assert_eq!(res, Ok(operation::Output::Record(None)));

    let get = operation::GetBorrowed {
        namespace: namespace(0, 0),
        key: b"foo",
        keyspace_version: Some(1),
    };

    let res = ctx.conn.execute(operation::Borrowed::Get(get).into()).await;
    assert_eq!(res, Ok(operation::Output::Record(None)));
}

#[tokio::test]
async fn accepts_only_new_keyspace_version_after_time_skew_window() {
    let ctx = Context::new().await;
    ctx.start_migration().await;

    let settings = *ctx.replica.cluster.view().settings();
    tokio::time::sleep(settings.event_propagation_latency + settings.clock_skew).await;

    let get = operation::GetBorrowed {
        namespace: namespace(0, 0),
        key: b"foo",
        keyspace_version: Some(0),
    };

    let res = ctx.conn.execute(operation::Borrowed::Get(get).into()).await;
    assert_eq!(res, Err(Error::keyspace_version_mismatch()));

    let get = operation::GetBorrowed {
        namespace: namespace(0, 0),
        key: b"foo",
        keyspace_version: Some(1),
    };

    let res = ctx.conn.execute(operation::Borrowed::Get(get).into()).await;
    assert_eq!(res, Ok(operation::Output::Record(None)));
}

#[tokio::test]
async fn forwards_storage_operations_to_database() {
    let ctx = Context::new().await;
    let ns = namespace(0, 0);

    let set = operation::Set {
        namespace: ns,
        key: b"foo".into(),
        record: Record {
            value: b"bar".into(),
            expiration: Duration::from_secs(30).into(),
            version: RecordVersion::now(),
        },
        keyspace_version: None,
    };

    let res = ctx.config.database.execute(set.clone().into()).await;
    assert_eq!(res, Ok(operation::Output::none()));

    let get = operation::GetBorrowed {
        namespace: namespace(0, 0),
        key: b"foo",
        keyspace_version: Some(0),
    };

    let res = ctx.conn.execute(operation::Borrowed::Get(get).into()).await;
    assert_eq!(res, Ok(operation::Output::Record(Some(set.record))));
}

#[tokio::test]
async fn does_not_accept_read_data_without_migration() {
    let ctx = Context::new().await;

    assert_eq!(
        ctx.conn.read_data(0..=u64::MAX, 0).await.map(drop),
        Err(Error::keyspace_version_mismatch())
    )
}

#[tokio::test]
async fn does_not_accept_read_data_after_migration_start() {
    let ctx = Context::new().await;

    ctx.start_migration().await;

    assert_eq!(
        ctx.conn.read_data(0..=u64::MAX, 1).await.map(drop),
        Err(Error::keyspace_version_mismatch())
    )
}

#[tokio::test]
async fn does_not_accept_read_data_after_event_propagation_delay() {
    let ctx = Context::new().await;

    let settings = *ctx.replica.cluster.view().settings();

    ctx.start_migration().await;
    tokio::time::sleep(settings.event_propagation_latency).await;

    assert_eq!(
        ctx.conn.read_data(0..=u64::MAX, 1).await.map(drop),
        Err(Error::keyspace_version_mismatch())
    )
}

#[tokio::test]
async fn does_not_accept_read_data_after_event_propagation_and_time_skew_delay() {
    let ctx = Context::new().await;

    let settings = *ctx.replica.cluster.view().settings();

    ctx.start_migration().await;
    tokio::time::sleep(settings.event_propagation_latency + settings.clock_skew).await;

    assert_eq!(
        ctx.conn.read_data(0..=u64::MAX, 1).await.map(drop),
        Err(Error::keyspace_version_mismatch())
    )
}

#[tokio::test]
async fn accepts_read_data_only_after_event_propagation_and_time_skew_delay_plus_leeway_time() {
    let ctx = Context::new().await;

    let settings = *ctx.replica.cluster.view().settings();

    ctx.start_migration().await;

    tokio::time::sleep(
        settings.event_propagation_latency + settings.clock_skew + Duration::from_secs(2),
    )
    .await;

    assert_eq!(ctx.conn.read_data(0..=u64::MAX, 1).await.map(drop), Ok(()))
}

#[tokio::test]
async fn forwards_read_data_to_database() {
    let ctx = Context::new().await;
    let ns = namespace(0, 0);

    for n in 0..10 {
        let set = operation::Set {
            namespace: ns,
            key: vec![n],
            record: Record {
                value: b"bar".into(),
                expiration: Duration::from_secs(30).into(),
                version: RecordVersion::now(),
            },
            keyspace_version: None,
        };

        let res = ctx.config.database.execute(set.clone().into()).await;
        assert_eq!(res, Ok(operation::Output::none()));
    }

    ctx.start_migration().await;

    let settings = *ctx.replica.cluster.view().settings();
    tokio::time::sleep(
        settings.event_propagation_latency + settings.clock_skew + Duration::from_secs(2),
    )
    .await;

    let items: Vec<_> = ctx
        .conn
        .read_data(0..=u64::MAX, 1)
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();

    assert_eq!(items.len(), 10 + 1); // +1 `Done` frame}
}
