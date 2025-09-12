use {
    alloy::{
        node_bindings::{Anvil, AnvilInstance},
        signers::local::PrivateKeySigner,
    },
    derive_more::derive::AsRef,
    futures::{StreamExt, stream},
    futures_concurrency::future::Race as _,
    libp2p_identity::Keypair,
    metrics_exporter_prometheus::PrometheusRecorder,
    std::{
        collections::HashSet,
        net::{Ipv4Addr, SocketAddrV4, TcpListener},
        thread,
        time::Duration,
    },
    tap::Pipe,
    wcn_client::{ClientBuilder, PeerId, ReplicaClient},
    wcn_cluster::{
        Cluster,
        EncryptionKey,
        Settings,
        keyspace::ReplicationStrategy,
        migration,
        node_operator,
        smart_contract::{
            self,
            evm::{self, RpcProvider, Signer},
        },
    },
    wcn_rpc::server::ShutdownSignal,
    wcn_storage_api::Namespace,
};

mod load_simulator;

const OPERATORS_COUNT: u8 = 8;

#[derive(AsRef, Clone, Copy)]
struct DeploymentConfig {
    #[as_ref]
    encryption_key: EncryptionKey,
}

impl wcn_cluster::Config for DeploymentConfig {
    type SmartContract = evm::SmartContract;
    type KeyspaceShards = ();
    type Node = wcn_cluster::Node;

    fn new_node(&self, _operator_id: node_operator::Id, node: wcn_cluster::Node) -> Self::Node {
        node
    }
}

pub struct TestCluster {
    operators: Vec<NodeOperator>,

    cluster: Cluster<DeploymentConfig>,
    anvil: AnvilInstance,

    next_operator_id: u8,
}

impl Drop for TestCluster {
    fn drop(&mut self) {
        for operator in &self.operators {
            operator.database.shutdown_signal.emit();

            for node in &operator.nodes {
                node.shutdown_signal.emit();
            }
        }
    }
}

impl TestCluster {
    pub async fn deploy() -> Self {
        // Spin up a local Anvil instance automatically
        let anvil = Anvil::new()
            .block_time(1)
            .chain_id(31337)
            .args(["--accounts", "100"])
            .try_spawn()
            .unwrap();

        let settings = Settings {
            max_node_operator_data_bytes: 4096,
            event_propagation_latency: Duration::from_secs(1),
            clock_skew: Duration::from_millis(100),
        };

        tracing::info!(port = %anvil.port(), "Anvil launched");

        // Use Anvil's first key for deployment - convert PrivateKeySigner to our Signer
        let private_key_signer: PrivateKeySigner = anvil.keys().last().unwrap().clone().into();
        let signer =
            Signer::try_from_private_key(&format!("{:#x}", private_key_signer.to_bytes())).unwrap();

        let provider = provider(signer, &anvil).await;

        // dummy value, we don't know the address at this point yet
        let contract_address = "0xF85FA2ce74D0b65756E14377f0359BB13E229ECE"
            .parse()
            .unwrap();

        let mut operators: Vec<_> = (1..=OPERATORS_COUNT)
            .map(|n| NodeOperator::new(n, &anvil, contract_address))
            .collect();
        let operators_on_chain = operators.iter().map(NodeOperator::on_chain).collect();

        let encryption_key = wcn_cluster::testing::encryption_key();
        let cfg = DeploymentConfig { encryption_key };

        let cluster = Cluster::deploy(cfg, &provider, settings, operators_on_chain)
            .await
            .unwrap();

        let contract_address = cluster.smart_contract().address();

        operators
            .iter_mut()
            .flat_map(|operator| operator.nodes.as_mut_slice())
            .for_each(|node| node.config.smart_contract_address = contract_address);

        stream::iter(&mut operators)
            .for_each_concurrent(OPERATORS_COUNT as usize, NodeOperator::deploy)
            .await;

        Self {
            operators,
            cluster,
            anvil,
            next_operator_id: OPERATORS_COUNT + 1,
        }
    }

    pub async fn under_load(&mut self, f: impl AsyncFnOnce(&mut Self)) {
        let operator = &mut self.operators[0];
        operator.initialize_clients().await;

        let client = operator.clients[0].inner.clone().unwrap();
        let namespaces = [operator.namespace(0), operator.namespace(1)];

        (load_simulator::run(client, &namespaces), f(self))
            .race()
            .await
    }

    pub async fn shutdown_one_node_per_node_operator(&mut self) {
        tracing::info!("Shutting down one node of each node operator");

        for operator in &mut self.operators {
            operator.nodes[0].shutdown().await;
        }
    }

    pub async fn redeploy_all_offline_nodes(&mut self) {
        tracing::info!("Redeploying all offline nodes");

        for operator in &mut self.operators {
            for node in &mut operator.nodes {
                if node.is_offline() {
                    node.deploy().await;
                }
            }
        }
    }

    pub async fn replace_all_node_operators_except_namespace_owner(&mut self) {
        tracing::info!("Replacing all node operators, except the namespace owner");

        let contract_address = self.cluster.smart_contract().address();

        for _ in 0..self.operators.len() - 1 {
            let idx = 1;

            let operator_id = *self.operators[idx].signer.address();
            tracing::info!(%operator_id, "Removing node operator from the cluster");

            let migration_plan = migration::Plan {
                remove: [operator_id].into_iter().collect(),
                add: HashSet::default(),
                replication_strategy: ReplicationStrategy::UniformDistribution,
            };

            self.cluster.start_migration(migration_plan).await.unwrap();
            self.wait_no_migration().await;

            self.cluster
                .remove_node_operator(operator_id)
                .await
                .unwrap();

            self.operators[idx].shutdown().await;
            self.operators.remove(idx);

            let mut operator =
                NodeOperator::new(self.next_operator_id(), &self.anvil, contract_address);

            let operator_id = *operator.signer.address();
            tracing::info!(%operator_id, "Adding node operator to the cluster");

            self.cluster
                .add_node_operator(operator.on_chain())
                .await
                .unwrap();

            operator.deploy().await;
            self.operators.push(operator);

            let migration_plan = migration::Plan {
                remove: HashSet::default(),
                add: [operator_id].into_iter().collect(),
                replication_strategy: ReplicationStrategy::UniformDistribution,
            };

            self.cluster.start_migration(migration_plan).await.unwrap();
            self.wait_no_migration().await;
        }
    }

    pub fn authorized_client(&self) -> &Client {
        &self.operators[0].clients[0]
    }

    pub fn node_operator(&self, id: node_operator::Id) -> Option<&NodeOperator> {
        self.operators.iter().find(|op| op.signer.address() == &id)
    }

    fn next_operator_id(&mut self) -> u8 {
        let id = self.next_operator_id;
        self.next_operator_id += 1;
        id
    }

    async fn wait_no_migration(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            if self.cluster.using_view(|view| view.migration().is_none()) {
                return;
            }

            interval.tick().await;
        }
    }
}

pub struct NodeOperator {
    signer: Signer,
    name: node_operator::Name,
    database: Database,
    nodes: Vec<Node>,
    clients: Vec<Client>,
}

pub struct Client {
    operator_id: node_operator::Id,

    keypair: Keypair,
    authorized_namespaces: Vec<u8>,

    inner: Option<WcnClient>,
}

type WcnClient = ReplicaClient<wcn_client::WithRetries<wcn_client::WithEncryption>>;

impl Client {
    pub fn operator_id(&self) -> node_operator::Id {
        self.operator_id
    }

    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    pub fn authorized_namespace(&self) -> Namespace {
        // first 2 are reserved for load simulator
        namespace(self.operator_id, 2)
    }

    fn on_chain(&self) -> wcn_cluster::Client {
        wcn_cluster::Client {
            peer_id: self.keypair.public().to_peer_id(),
            authorized_namespaces: self.authorized_namespaces.clone().into(),
        }
    }

    async fn initialize(&mut self, nodes: &[Node]) {
        let nodes = nodes
            .iter()
            .map(|node| wcn_client::PeerAddr {
                id: node.config.keypair.public().to_peer_id(),
                addr: SocketAddrV4::new(
                    Ipv4Addr::LOCALHOST,
                    node.config.primary_rpc_server_socket.port().unwrap(),
                ),
            })
            .collect();

        let encryption_secret: [u8; 32] = std::array::from_fn(|idx| idx as u8);

        self.inner = wcn_client::Client::new(wcn_client::Config {
            keypair: self.keypair.clone(),
            cluster_key: wcn_cluster::testing::encryption_key(),
            connection_timeout: Duration::from_secs(1),
            operation_timeout: Duration::from_secs(2),
            reconnect_interval: Duration::from_millis(100),
            max_concurrent_rpcs: 5000,
            max_idle_connection_timeout: Duration::from_secs(5),
            nodes,
            metrics_tag: "integration_tests",
        })
        .await
        .unwrap()
        .with_encryption(wcn_client::EncryptionKey::new(&encryption_secret).unwrap())
        .with_retries(3)
        .build()
        .pipe(Some);
    }
}

struct Database {
    operator_id: node_operator::Id,
    config: wcn_db::Config,
    _prometheus_recorder: PrometheusRecorder,
    shutdown_signal: ShutdownSignal,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl Database {
    pub fn peer_id(&self) -> PeerId {
        self.config.keypair.public().to_peer_id()
    }

    fn deploy(&mut self) {
        let operator_id = self.operator_id;
        tracing::info!(%operator_id, peer_id = %self.peer_id(), "Deploying database");

        let fut = wcn_db::run(clone_database_config(&self.config)).unwrap();

        self.thread_handle = Some(thread::spawn(move || {
            let _guard = tracing::info_span!("database", %operator_id).entered();
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(fut);
        }));
        tracing::info!(%operator_id, peer_id = %self.peer_id(), "Database deployed");
    }

    async fn shutdown(&mut self) {
        tracing::info!(operator_id = %self.operator_id, "Shutting down database");

        self.shutdown_signal.emit();

        let thread_handle = self.thread_handle.take().unwrap();

        tokio::task::spawn_blocking(move || thread_handle.join())
            .await
            .unwrap()
            .unwrap();

        tracing::info!(operator_id = %self.operator_id, "Database shut down");
    }
}

pub struct Node {
    operator_id: node_operator::Id,
    config: wcn_node::Config,
    _prometheus_recorder: PrometheusRecorder,
    shutdown_signal: ShutdownSignal,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl Node {
    pub fn peer_id(&self) -> PeerId {
        self.config.keypair.public().to_peer_id()
    }

    pub fn primary_rpc_server_port(&self) -> u16 {
        self.config.primary_rpc_server_socket.port().unwrap()
    }

    async fn deploy(&mut self) {
        let operator_id = self.operator_id;
        tracing::info!(%operator_id, peer_id = %self.peer_id(), "Deploying node");

        self.shutdown_signal = ShutdownSignal::new();
        let mut cfg = clone_node_config(&self.config);
        cfg.shutdown_signal = self.shutdown_signal.clone();

        let fut = wcn_node::run(cfg);

        let (tx, rx) = std::sync::mpsc::channel::<wcn_node::Result<()>>();

        self.thread_handle = Some(thread::spawn(move || {
            let _guard = tracing::info_span!("node", %operator_id).entered();
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    match fut.await {
                        Ok(fut) => {
                            let _ = tx.send(Ok(()));
                            fut.await
                        }
                        Err(err) => {
                            tx.send(Err(err)).pipe(drop);
                        }
                    }
                });
        }));

        tokio::task::spawn_blocking(move || rx.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        tracing::info!(operator_id = %self.operator_id, node_id = %self.peer_id(), "Node deployed");
    }

    async fn shutdown(&mut self) {
        tracing::info!(operator_id = %self.operator_id, node_id = %self.peer_id(), "Shutting down node");

        self.shutdown_signal.emit();

        let thread_handle = self.thread_handle.take().unwrap();

        tokio::task::spawn_blocking(move || thread_handle.join())
            .await
            .unwrap()
            .unwrap();

        tracing::info!(operator_id = %self.operator_id, node_id = %self.peer_id(), "Node shut down");
    }

    fn is_offline(&self) -> bool {
        self.thread_handle.is_none()
    }

    fn on_chain(&self) -> wcn_cluster::Node {
        wcn_cluster::Node {
            peer_id: self.config.keypair.public().to_peer_id(),
            ipv4_addr: Ipv4Addr::LOCALHOST,
            primary_port: self.config.primary_rpc_server_socket.port().unwrap(),
            secondary_port: self.config.secondary_rpc_server_socket.port().unwrap(),
        }
    }
}

impl NodeOperator {
    fn new(
        id: u8,
        anvil: &AnvilInstance,
        contract_address: smart_contract::Address,
    ) -> NodeOperator {
        let smart_contract_signer = anvil.keys()[id as usize]
            .to_bytes()
            .pipe(|bytes| Signer::try_from_private_key(&const_hex::encode(bytes)).unwrap());

        let operator_id = *smart_contract_signer.address();

        let rpc_provider_url = anvil.endpoint_url().to_string().replace("http://", "ws://");

        let db_keypair = Keypair::generate_ed25519();
        let db_peer_id = db_keypair.public().to_peer_id();
        let db_shutdown_signal = ShutdownSignal::new();

        let db_prometheus_recorder =
            metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();

        let db_config = wcn_db::Config {
            keypair: db_keypair,
            primary_rpc_server_socket: wcn_rpc::server::Socket::new_high_priority(0).unwrap(),
            secondary_rpc_server_socket: wcn_rpc::server::Socket::new_low_priority(0).unwrap(),
            metrics_server_socket: new_tcp_socket(),
            connection_timeout: Duration::from_secs(1),
            max_connections: 100,
            max_connections_per_ip: 100,
            max_connection_rate_per_ip: 100,
            max_concurrent_rpcs: 20000,
            max_idle_connection_timeout: Duration::from_millis(500),
            rocksdb_dir: format!("/tmp/wcn_db/{db_peer_id}").parse().unwrap(),
            rocksdb: Default::default(),
            shutdown_signal: db_shutdown_signal.clone(),
            prometheus_handle: db_prometheus_recorder.handle(),
        };

        let database_primary_rpc_server_port = db_config.primary_rpc_server_socket.port().unwrap();
        let database_secondary_rpc_server_port =
            db_config.secondary_rpc_server_socket.port().unwrap();

        let database = Database {
            config: db_config,
            _prometheus_recorder: db_prometheus_recorder,
            shutdown_signal: db_shutdown_signal,
            thread_handle: None,
            operator_id,
        };

        let nodes = (0..=1)
            .map(|n| {
                let keypair = Keypair::generate_ed25519();
                let shutdown_signal = ShutdownSignal::new();

                let prometheus_recorder =
                    metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();

                let config = wcn_node::Config {
                    keypair,
                    primary_rpc_server_socket: wcn_rpc::server::Socket::new_high_priority(0)
                        .unwrap(),
                    secondary_rpc_server_socket: wcn_rpc::server::Socket::new_low_priority(0)
                        .unwrap(),
                    max_idle_connection_timeout: Duration::from_secs(5),
                    metrics_server_socket: new_tcp_socket(),
                    database_rpc_server_address: Ipv4Addr::LOCALHOST,
                    database_peer_id: db_peer_id,
                    database_primary_rpc_server_port,
                    database_secondary_rpc_server_port,
                    smart_contract_address: contract_address,
                    smart_contract_signer: (n == 0).then_some(smart_contract_signer.clone()),
                    smart_contract_encryption_key: wcn_cluster::testing::encryption_key(),
                    rpc_provider_url: rpc_provider_url.clone().parse().unwrap(),
                    shutdown_signal: shutdown_signal.clone(),
                    prometheus_handle: prometheus_recorder.handle(),
                };

                Node {
                    operator_id,
                    config,
                    _prometheus_recorder: prometheus_recorder,
                    shutdown_signal,
                    thread_handle: None,
                }
            })
            .collect();

        Self {
            signer: smart_contract_signer,
            name: node_operator::Name::new(format!("operator{id}")).unwrap(),
            database,
            nodes,
            clients: vec![Client {
                operator_id,
                keypair: Keypair::generate_ed25519(),
                authorized_namespaces: if id == 1 { vec![0, 1, 2] } else { vec![] },
                inner: None,
            }],
        }
    }

    async fn deploy(&mut self) {
        self.database.deploy();

        self.nodes
            .iter_mut()
            .pipe(stream::iter)
            .for_each_concurrent(10, Node::deploy)
            .await;
    }

    async fn shutdown(&mut self) {
        self.nodes
            .iter_mut()
            .pipe(stream::iter)
            .for_each_concurrent(10, Node::shutdown)
            .await;

        self.database.shutdown().await;
    }

    async fn initialize_clients(&mut self) {
        stream::iter(self.clients.iter_mut())
            .for_each_concurrent(5, |client| client.initialize(&self.nodes))
            .await
    }

    fn on_chain(&self) -> wcn_cluster::NodeOperator {
        wcn_cluster::NodeOperator::new(
            *self.signer.address(),
            self.name.clone(),
            self.nodes.iter().map(Node::on_chain).collect(),
            self.clients.iter().map(Client::on_chain).collect(),
        )
        .unwrap()
    }

    fn namespace(&self, idx: u8) -> Namespace {
        namespace(*self.signer.address(), idx)
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
}

fn namespace(operator_id: node_operator::Id, idx: u8) -> Namespace {
    format!("{operator_id}/{idx}").parse().unwrap()
}

async fn provider(signer: Signer, anvil: &AnvilInstance) -> RpcProvider {
    let ws_url = anvil.endpoint_url().to_string().replace("http://", "ws://");
    RpcProvider::new(ws_url.parse().unwrap(), signer)
        .await
        .unwrap()
}

fn new_tcp_socket() -> TcpListener {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).unwrap()
}

fn clone_node_config(cfg: &wcn_node::Config) -> wcn_node::Config {
    wcn_node::Config {
        keypair: cfg.keypair.clone(),
        primary_rpc_server_socket: cfg.primary_rpc_server_socket.try_clone().unwrap(),
        secondary_rpc_server_socket: cfg.secondary_rpc_server_socket.try_clone().unwrap(),
        max_idle_connection_timeout: Duration::from_secs(5),
        metrics_server_socket: cfg.metrics_server_socket.try_clone().unwrap(),
        database_rpc_server_address: cfg.database_rpc_server_address,
        database_peer_id: cfg.database_peer_id,
        database_primary_rpc_server_port: cfg.database_primary_rpc_server_port,
        database_secondary_rpc_server_port: cfg.database_secondary_rpc_server_port,
        smart_contract_address: cfg.smart_contract_address,
        smart_contract_signer: cfg.smart_contract_signer.clone(),
        smart_contract_encryption_key: cfg.smart_contract_encryption_key,
        rpc_provider_url: cfg.rpc_provider_url.clone(),
        shutdown_signal: cfg.shutdown_signal.clone(),
        prometheus_handle: cfg.prometheus_handle.clone(),
    }
}

fn clone_database_config(cfg: &wcn_db::Config) -> wcn_db::Config {
    wcn_db::Config {
        keypair: cfg.keypair.clone(),
        primary_rpc_server_socket: cfg.primary_rpc_server_socket.try_clone().unwrap(),
        secondary_rpc_server_socket: cfg.secondary_rpc_server_socket.try_clone().unwrap(),
        metrics_server_socket: cfg.metrics_server_socket.try_clone().unwrap(),
        connection_timeout: cfg.connection_timeout,
        max_connections: cfg.max_connections,
        max_connections_per_ip: cfg.max_connections_per_ip,
        max_connection_rate_per_ip: cfg.max_connection_rate_per_ip,
        max_concurrent_rpcs: cfg.max_concurrent_rpcs,
        max_idle_connection_timeout: cfg.max_idle_connection_timeout,
        rocksdb_dir: cfg.rocksdb_dir.clone(),
        rocksdb: cfg.rocksdb.clone(),
        shutdown_signal: cfg.shutdown_signal.clone(),
        prometheus_handle: cfg.prometheus_handle.clone(),
    }
}
