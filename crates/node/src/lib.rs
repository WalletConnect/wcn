use {
    derive_more::AsRef,
    derive_where::derive_where,
    futures::{future::OptionFuture, FutureExt as _, StreamExt as _},
    futures_concurrency::future::Join as _,
    libp2p_identity::Keypair,
    std::{
        future::Future,
        io,
        net::{Ipv4Addr, SocketAddrV4, TcpListener},
        sync::Arc,
        time::Duration,
    },
    tap::Pipe,
    wc::metrics::exporter_prometheus::PrometheusHandle,
    wcn_cluster::{
        keyspace,
        node_operator,
        smart_contract::{self, evm},
        Cluster,
        EncryptionKey,
        PeerId,
    },
    wcn_metrics_api::MetricsApi,
    wcn_rpc::{
        client::{self as rpc_client, Api as _},
        server::{self as rpc_server, Api as _, Server as _, ShutdownSignal},
        transport::BandwidthLimiter,
    },
    wcn_storage_api::rpc::client as storage_api_client,
};

/// Configuration options of a WCN Node.
#[derive_where(Debug)]
pub struct Config {
    /// [`Keypair`] of the node.
    pub keypair: Keypair,

    /// Socket to bind the primary WCN RPC server to.
    pub primary_rpc_server_socket: wcn_rpc::server::Socket,

    /// Socket to bind the secondary WCN RPC server to.
    pub secondary_rpc_server_socket: wcn_rpc::server::Socket,

    /// Socket to serve the Prometheus metrics server on.
    pub metrics_server_socket: TcpListener,

    /// For how long an inbound connection is allowed to be idle before it
    /// gets timed out.
    pub max_idle_connection_timeout: Duration,

    /// [`Ipv4Addr`] of the local WCN Database to connect to.
    ///
    /// WCN Databases don't have authorization and MUST not be exposed to the
    /// public internet. So, this address should be `127.0.0.1` or `10.a.b.c`
    /// etc.
    pub database_rpc_server_address: Ipv4Addr,

    /// [`PeerId`] of the local WCN Database to connect to.
    // TODO: We don't need server auth for this, but it's baked into the RPC machinery rn.
    pub database_peer_id: PeerId,

    /// Port of the primary WCN RPC server of the local WCN Database to
    /// connect to.
    pub database_primary_rpc_server_port: u16,

    /// Port of the secondary WCN RPC server of the local WCN Database to
    /// connect to.
    pub database_secondary_rpc_server_port: u16,

    /// Address of the WCN Cluster Smart-Contract.
    pub smart_contract_address: smart_contract::Address,

    /// Signer to be used for the WCN Cluster Smart-Contract calls.
    ///
    /// Only one node within node operator infrastructure is supposed to have
    /// this set.
    pub smart_contract_signer: Option<smart_contract::evm::Signer>,

    /// [`EncryptioKey`] used to encrypt/decrypt on-chain data.
    ///
    /// It's a symmetric key that's meant to be shared across all node
    /// operators.
    pub smart_contract_encryption_key: EncryptionKey,

    /// URL of the Optimism RPC provider.
    pub rpc_provider_url: smart_contract::evm::RpcUrl,

    /// [`ShutdownSignal`] to use for RPC servers.
    pub shutdown_signal: ShutdownSignal,

    /// [`PrometheusHandle`] to use for getting metrics in metrics server.
    #[derive_where(skip)]
    pub prometheus_handle: PrometheusHandle,

    /// [`Ipv4Addr`] of this node.
    pub public_address: Option<Ipv4Addr>,
}

impl Config {
    fn coordinator_api(&self) -> wcn_storage_api::rpc::CoordinatorApi {
        wcn_storage_api::rpc::CoordinatorApi::new().with_rpc_timeout(Duration::from_secs(2))
    }

    fn replica_api(&self) -> wcn_storage_api::rpc::ReplicaApi {
        wcn_storage_api::rpc::ReplicaApi::new().with_rpc_timeout(Duration::from_secs(2))
    }

    fn database_api(&self) -> wcn_storage_api::rpc::DatabaseApi {
        wcn_storage_api::rpc::DatabaseApi::new().with_rpc_timeout(Duration::from_millis(500))
    }

    fn cluster_api(&self) -> wcn_cluster_api::rpc::ClusterApi {
        wcn_cluster_api::rpc::ClusterApi::new().with_rpc_timeout(Duration::from_secs(5))
    }

    fn metrics_api(&self) -> wcn_metrics_api::rpc::MetricsApi {
        wcn_metrics_api::rpc::MetricsApi::new().with_rpc_timeout(Duration::from_secs(5))
    }

    fn try_into_app(&self) -> Result<AppConfig, ErrorInner> {
        let migration_tx_bandwidth_limiter = BandwidthLimiter::new(0);
        let migration_rx_bandwidth_limiter = BandwidthLimiter::new(0);

        let replica_client_cfg = rpc_client::Config {
            keypair: self.keypair.clone(),
            connection_timeout: Duration::from_secs(2),
            reconnect_interval: Duration::from_millis(100),
            max_concurrent_rpcs: 10000,
            max_idle_connection_timeout: Duration::from_millis(200),
            priority: wcn_rpc::transport::Priority::High,
        };

        let replica_low_prio_client_cfg = rpc_client::Config {
            keypair: self.keypair.clone(),
            connection_timeout: Duration::from_secs(10),
            reconnect_interval: Duration::from_secs(1),
            max_concurrent_rpcs: 200,
            max_idle_connection_timeout: self.max_idle_connection_timeout,
            priority: wcn_rpc::transport::Priority::Low,
        };

        let database_client_cfg = rpc_client::Config {
            keypair: self.keypair.clone(),
            connection_timeout: Duration::from_secs(1),
            reconnect_interval: Duration::from_millis(100),
            max_concurrent_rpcs: 5000,
            max_idle_connection_timeout: self.max_idle_connection_timeout,
            priority: wcn_rpc::transport::Priority::High,
        };

        let database_low_prio_client_cfg = rpc_client::Config {
            keypair: self.keypair.clone(),
            connection_timeout: Duration::from_secs(1),
            reconnect_interval: Duration::from_millis(100),
            max_concurrent_rpcs: 200,
            max_idle_connection_timeout: self.max_idle_connection_timeout,
            priority: wcn_rpc::transport::Priority::Low,
        };

        let metrics_client_cfg = rpc_client::Config {
            keypair: self.keypair.clone(),
            connection_timeout: Duration::from_secs(5),
            reconnect_interval: Duration::from_millis(1),
            max_concurrent_rpcs: 300,
            max_idle_connection_timeout: self.max_idle_connection_timeout,
            priority: wcn_rpc::transport::Priority::Low,
        };

        let database_client = self.database_api().try_into_client(database_client_cfg)?;
        let database_low_prio_client = self
            .database_api()
            .with_tx_bandwidth_limit(migration_tx_bandwidth_limiter.clone())
            .with_rx_bandwidth_limit(migration_rx_bandwidth_limiter.clone())
            .try_into_client(database_low_prio_client_cfg)?;

        let metrics_client = self.metrics_api().try_into_client(metrics_client_cfg)?;

        Ok(AppConfig {
            encryption_key: self.smart_contract_encryption_key,

            replica_client: self.replica_api().try_into_client(replica_client_cfg)?,
            replica_low_prio_client: self
                .replica_api()
                .with_tx_bandwidth_limit(migration_tx_bandwidth_limiter.clone())
                .with_rx_bandwidth_limit(migration_rx_bandwidth_limiter.clone())
                .try_into_client(replica_low_prio_client_cfg)?,

            database_connection: database_client.new_connection(
                self.database_primary_socket_addr(),
                &self.database_peer_id,
                (),
            ),

            database_low_prio_connection: database_low_prio_client.new_connection(
                self.database_secondary_socket_addr(),
                &self.database_peer_id,
                (),
            ),

            database_metrics_connection: metrics_client.new_connection(
                self.database_secondary_socket_addr(),
                &self.database_peer_id,
                (),
            ),
            metrics_client,

            migration_tx_bandwidth_limiter,
            migration_rx_bandwidth_limiter,

            public_address: self.public_address,
        })
    }

    fn database_primary_socket_addr(&self) -> SocketAddrV4 {
        SocketAddrV4::new(
            self.database_rpc_server_address,
            self.database_primary_rpc_server_port,
        )
    }

    fn database_secondary_socket_addr(&self) -> SocketAddrV4 {
        SocketAddrV4::new(
            self.database_rpc_server_address,
            self.database_secondary_rpc_server_port,
        )
    }
}

#[derive(AsRef, Clone)]
struct AppConfig {
    #[as_ref]
    encryption_key: EncryptionKey,

    replica_client: storage_api_client::Replica,
    replica_low_prio_client: storage_api_client::Replica,

    database_connection: storage_api_client::DatabaseConnection,
    database_low_prio_connection: storage_api_client::DatabaseConnection,

    database_metrics_connection: wcn_metrics_api::rpc::client::Connection,
    metrics_client: wcn_metrics_api::rpc::Client,

    migration_tx_bandwidth_limiter: BandwidthLimiter,
    migration_rx_bandwidth_limiter: BandwidthLimiter,

    public_address: Option<Ipv4Addr>,
}

#[derive(AsRef, Clone)]
struct Node {
    #[as_ref]
    peer_id: PeerId,

    #[as_ref]
    replica_connection: storage_api_client::ReplicaConnection,

    replica_low_prio_connection: storage_api_client::ReplicaConnection,

    metrics_connection: wcn_metrics_api::rpc::client::Connection,
}

impl wcn_cluster::Config for AppConfig {
    type SmartContract = evm::SmartContract;
    type KeyspaceShards = keyspace::Shards;
    type Node = Node;

    fn new_node(&self, _operator_id: node_operator::Id, node: wcn_cluster::Node) -> Self::Node {
        fn same_socket_addr(ip: &Ipv4Addr, port: u16, expected_socket_addr: &SocketAddrV4) -> bool {
            ip == expected_socket_addr.ip() && port == expected_socket_addr.port()
        }
        let primary_socket_addr = node.primary_socket_addr();
        let primary_socket_addr = match &self.public_address {
            Some(addr) if same_socket_addr(addr, node.primary_port, &primary_socket_addr) => {
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, primary_socket_addr.port())
            }
            _ => primary_socket_addr,
        };
        let secondary_socket_addr = node.secondary_socket_addr();
        let secondary_socket_addr = match &self.public_address {
            Some(addr) if same_socket_addr(addr, node.secondary_port, &secondary_socket_addr) => {
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, secondary_socket_addr.port())
            }
            _ => secondary_socket_addr,
        };
        Node {
            peer_id: node.peer_id,
            replica_connection: self.replica_client.new_connection(
                primary_socket_addr,
                &node.peer_id,
                (),
            ),
            replica_low_prio_connection: self.replica_low_prio_client.new_connection(
                secondary_socket_addr,
                &node.peer_id,
                (),
            ),
            metrics_connection: self.metrics_client.new_connection(
                secondary_socket_addr,
                &node.peer_id,
                (),
            ),
        }
    }

    fn update_settings(&self, settings: &wcn_cluster::Settings) {
        self.migration_tx_bandwidth_limiter
            .set_bps(settings.migration_tx_bandwidth);

        self.migration_rx_bandwidth_limiter
            .set_bps(settings.migration_rx_bandwidth);
    }
}

impl wcn_replication::coordinator::Config for AppConfig {
    type OutboundReplicaConnection = storage_api_client::ReplicaConnection;
}

impl wcn_replication::replica::Config for AppConfig {
    type OutboundDatabaseConnection = storage_api_client::DatabaseConnection;
}

impl wcn_migration::manager::Config for AppConfig {
    type OutboundReplicaConnection = storage_api_client::ReplicaConnection;
    type OutboundDatabaseConnection = storage_api_client::DatabaseConnection;

    fn get_replica_connection<'a>(
        &self,
        node: &'a Self::Node,
    ) -> &'a Self::OutboundReplicaConnection {
        &node.replica_low_prio_connection
    }
}

impl wcn_metrics::aggregator::Config for AppConfig {
    fn outbound_metrics_api_connection(node: &Self::Node) -> &impl MetricsApi {
        &node.metrics_connection
    }
}

pub async fn run(config: Config) -> Result<impl Future<Output = ()>> {
    run_(config).await.map_err(Into::into)
}

async fn run_(config: Config) -> Result<impl Future<Output = ()>, ErrorInner> {
    let app_cfg = config.try_into_app()?;

    let rpc_provider = if let Some(signer) = config.smart_contract_signer.clone() {
        smart_contract::evm::RpcProvider::new(config.rpc_provider_url.clone(), signer).await
    } else {
        smart_contract::evm::RpcProvider::new_ro(config.rpc_provider_url.clone()).await
    }?;

    let cluster = Cluster::connect(
        app_cfg.clone(),
        &rpc_provider,
        config.smart_contract_address,
    )
    .await?;

    cluster.using_view(|view| {
        let settings = view.settings();

        app_cfg
            .migration_tx_bandwidth_limiter
            .set_bps(settings.migration_tx_bandwidth);

        app_cfg
            .migration_rx_bandwidth_limiter
            .set_bps(settings.migration_rx_bandwidth);
    });

    let coordinator = wcn_replication::Coordinator::new(app_cfg.clone(), cluster.clone());

    let replica = wcn_replication::Replica::new(
        Arc::new(app_cfg.clone()),
        cluster.clone(),
        app_cfg.database_connection.clone(),
    );

    let migration_manager = wcn_migration::Manager::new(
        app_cfg.clone(),
        cluster.clone(),
        app_cfg.database_low_prio_connection,
    );

    // Set higher RPC timeout as replication tasks are being handled within RPC
    // lifecycle.
    let coordinator_api = config
        .coordinator_api()
        .with_rpc_timeout(Duration::from_secs(10))
        .with_state(coordinator);

    let replica_api = config.replica_api().with_state(replica);
    let cluster_api = config
        .cluster_api()
        .with_state(cluster.smart_contract().clone());

    let metrics_provider = wcn_metrics::Provider::new([
        wcn_metrics::Target::remote("db", app_cfg.database_metrics_connection.clone()),
        wcn_metrics::Target::local("node", config.prometheus_handle.clone()),
    ])
    .with_auth(cluster.clone());

    let metrics_api = config.metrics_api().with_state(metrics_provider);

    let metrics_aggregator = wcn_metrics::Aggregator::<AppConfig>::new(cluster);

    let prometheus_server_fut = prometheus_server(&config, metrics_aggregator)?;
    let system_monitor_fut = wcn_metrics::system::Monitor::new(config.prometheus_handle)
        .run(config.shutdown_signal.wait_owned());

    let primary_rpc_server_cfg = wcn_rpc::server::Config {
        name: "primary",
        socket: config.primary_rpc_server_socket,
        keypair: config.keypair.clone(),
        connection_timeout: Duration::from_secs(2),
        max_connections: 2000,
        max_connections_per_ip: 1000,
        max_connection_rate_per_ip: 1000,
        max_concurrent_rpcs: 10000,
        max_idle_connection_timeout: config.max_idle_connection_timeout,
        shutdown_signal: config.shutdown_signal.clone(),
    };

    let secondary_rcp_server_cfg = wcn_rpc::server::Config {
        name: "secondary",
        socket: config.secondary_rpc_server_socket,
        keypair: config.keypair.clone(),
        connection_timeout: Duration::from_secs(2),
        max_connections: 800,
        max_connections_per_ip: 400,
        max_connection_rate_per_ip: 400,
        max_concurrent_rpcs: 1000,
        max_idle_connection_timeout: config.max_idle_connection_timeout,
        shutdown_signal: config.shutdown_signal.clone(),
    };

    let primary_rpc_server_fut = coordinator_api
        .clone()
        .multiplex(replica_api.clone())
        .multiplex(cluster_api)
        .serve(primary_rpc_server_cfg)?;

    let secondary_rpc_server_fut = coordinator_api
        .multiplex(replica_api)
        .multiplex(metrics_api)
        .serve(secondary_rcp_server_cfg)?;

    let migration_manager_fut = migration_manager
        .map(|manager| manager.run(async move { config.shutdown_signal.wait().await }))
        .pipe(OptionFuture::from);

    (
        primary_rpc_server_fut,
        secondary_rpc_server_fut,
        system_monitor_fut,
        prometheus_server_fut,
        migration_manager_fut,
    )
        .join()
        .map(drop)
        .pipe(Ok)
}

fn prometheus_server(
    cfg: &Config,
    metrics_aggregator: wcn_metrics::Aggregator<AppConfig>,
) -> io::Result<impl Future<Output = ()>> {
    let socket = cfg.metrics_server_socket.try_clone()?;
    let prometheus = cfg.prometheus_handle.clone();
    let shutdown_fut = cfg.shutdown_signal.wait_owned();

    let svc = axum::Router::new()
        .route(
            "/metrics",
            axum::routing::get(move || async move { prometheus.render() }),
        )
        .route("/metrics/cluster", axum::routing::get(cluster_metrics))
        .with_state(metrics_aggregator)
        .into_make_service();

    socket.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(socket)?;

    Ok(async {
        let _ = axum::serve(listener, svc)
            .with_graceful_shutdown(shutdown_fut)
            .await;
    })
}

async fn cluster_metrics(
    state: axum::extract::State<wcn_metrics::Aggregator<AppConfig>>,
) -> impl axum::response::IntoResponse {
    let metrics = state.0.render_cluster_metrics();
    axum::body::Body::from_stream(metrics.map::<Result<_, axum::BoxError>, _>(Ok))
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct Error(#[from] ErrorInner);

#[derive(Debug, thiserror::Error)]
pub enum ErrorInner {
    #[error("RPC client: {0}")]
    RpcClient(#[from] rpc_client::Error),

    #[error("Failed to create Smart-Contract RPC provider: {0}")]
    RpcProvider(#[from] smart_contract::evm::RpcProviderCreationError),

    #[error("Failed to connect to WCN Cluster: {0}")]
    ClusterConnection(#[from] wcn_cluster::ConnectionError),

    #[error("RPC server: {0}")]
    RpcServer(#[from] rpc_server::Error),

    #[error("Metrics server: {0}")]
    MetricsServer(#[from] io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
