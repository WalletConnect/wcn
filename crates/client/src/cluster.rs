use {
    crate::{Connector, Error, NodeData, PeerAddr},
    arc_swap::ArcSwap,
    derive_more::derive::AsRef,
    derive_where::derive_where,
    futures::{Stream, StreamExt as _, TryStreamExt as _, future::Either, stream},
    std::{
        collections::HashSet,
        marker::PhantomData,
        net::SocketAddrV4,
        sync::Arc,
        time::Duration,
    },
    tokio::sync::oneshot,
    wc::{
        future::FutureExt as _,
        metrics::{self, Gauge, StringLabel},
    },
    wcn_cluster::{
        EncryptionKey,
        PeerId,
        node_operator,
        smart_contract::{ReadError, ReadResult},
    },
    wcn_cluster_api::{
        ClusterApi as _,
        ClusterView,
        Event,
        Read,
        rpc::{
            ClusterApi,
            client::{Cluster as ClusterClient, ClusterConnection},
        },
    },
    wcn_storage_api::rpc::{
        CoordinatorApi,
        client::{Coordinator as CoordinatorClient, CoordinatorConnection},
    },
};

struct NodeMetrics {
    addr: SocketAddrV4,
}

impl NodeMetrics {
    fn new(addr: SocketAddrV4) -> Self {
        let this = Self { addr };
        this.meter().increment(1);
        this
    }

    fn meter(&self) -> &Gauge {
        metrics::gauge!("wcn_client_live_nodes",
            StringLabel<"remote_addr", SocketAddrV4> => &self.addr
        )
    }
}

impl Drop for NodeMetrics {
    fn drop(&mut self) {
        self.meter().decrement(1);
    }
}

#[derive(Clone)]
pub struct Node<D> {
    pub peer_id: PeerId,
    pub public_cluster_conn: ClusterConnection,
    pub public_coordinator_conn: CoordinatorConnection,
    pub private_cluster_conn: Option<ClusterConnection>,
    pub private_coordinator_conn: Option<CoordinatorConnection>,
    pub data: D,
    _metrics: Arc<NodeMetrics>,
}

impl<D> Node<D> {
    pub(crate) fn cluster_api(&self) -> Connector<ClusterApi> {
        Connector {
            public_conn: self.public_cluster_conn.clone(),
            private_conn: self.private_cluster_conn.clone(),
        }
    }

    pub(crate) fn coordinator_api(&self) -> Connector<CoordinatorApi> {
        Connector {
            public_conn: self.public_coordinator_conn.clone(),
            private_conn: self.private_coordinator_conn.clone(),
        }
    }
}

impl<D> AsRef<PeerId> for Node<D> {
    fn as_ref(&self) -> &PeerId {
        &self.peer_id
    }
}

#[derive(AsRef)]
#[derive_where(Clone)]
pub(super) struct Config<D> {
    #[as_ref]
    encryption_key: EncryptionKey,
    cluster_api: ClusterClient,
    coordinator_api: CoordinatorClient,
    _marker: PhantomData<D>,
}

impl<D> Config<D> {
    pub fn new(
        encryption_key: EncryptionKey,
        cluster_api: ClusterClient,
        coordinator_api: CoordinatorClient,
    ) -> Self {
        Self {
            encryption_key,
            cluster_api,
            coordinator_api,
            _marker: PhantomData,
        }
    }
}

impl<D> wcn_cluster::Config for Config<D>
where
    D: NodeData + Send + Sync + 'static,
{
    type SmartContract = SmartContract<D>;
    type KeyspaceShards = ();
    type Node = Node<D>;

    fn new_node(&self, operator_id: node_operator::Id, node: wcn_cluster::Node) -> Self::Node {
        let id = &node.peer_id;
        let public_addr = node.primary_socket_addr();

        let public_cluster_conn =
            self.cluster_api
                .new_connection(public_addr, id, (), "public_cluster");
        let public_coordinator_conn =
            self.coordinator_api
                .new_connection(public_addr, id, (), "public_coordinator");

        let (private_cluster_conn, private_coordinator_conn) = node
            .primary_socket_addr_private()
            .map(|addr| {
                let cluster_conn = self
                    .cluster_api
                    .new_connection(addr, id, (), "private_cluster");
                let coordinator_conn =
                    self.coordinator_api
                        .new_connection(addr, id, (), "private_coordinator");

                (cluster_conn, coordinator_conn)
            })
            .unzip();

        Node {
            peer_id: node.peer_id,
            public_cluster_conn,
            public_coordinator_conn,
            private_cluster_conn,
            private_coordinator_conn,
            data: D::new(&operator_id, &node),
            _metrics: Arc::new(NodeMetrics::new(public_addr)),
        }
    }

    fn update_settings(&self, _settings: &wcn_cluster::Settings) {}
}

async fn select_open_connection<D>(
    cluster: &ArcSwap<View<D>>,
    trusted_operators: &HashSet<node_operator::Id>,
) -> ClusterConnection
where
    D: NodeData,
{
    let cluster = cluster.load();
    let next_node = || cluster.node_operators().next().next_node().cluster_api();

    loop {
        // Empty trusted operators list means all nodes are allowed to be used.
        let conn = if trusted_operators.is_empty() {
            next_node()
        } else {
            // Try to find a white-listed node. If that fails, just use the next node.
            cluster
                .node_operators()
                .find_next_operator(|operator| {
                    trusted_operators
                        .contains(&operator.id)
                        .then(|| operator.next_node().cluster_api())
                })
                .unwrap_or_else(next_node)
        };

        let result = conn
            .wait_open()
            .with_timeout(Duration::from_millis(1500))
            .await;

        if let Ok(conn) = result {
            return conn.clone();
        }
    }
}

// Smart contract here is implemented as an enum because we intend to use the
// same [`wcn_cluster::Config`] for both clusters: initialized from a bootstrap
// cluster view, and from a dynamically updated cluster.
//
// The reason why we're using the same [`wcn_cluster::Config`] for both clusters
// is that [`wcn_cluster::View`] is also parametrized over this config, and we
// need them to be compatible.
#[allow(clippy::large_enum_variant)]
pub(crate) enum SmartContract<D: NodeData> {
    Static(ClusterView),
    Dynamic {
        trusted_operators: HashSet<node_operator::Id>,
        cluster_view: Arc<ArcSwap<View<D>>>,
    },
}

impl<D: NodeData> Read for SmartContract<D> {
    async fn cluster_view(&self) -> ReadResult<ClusterView> {
        match self {
            Self::Static(view) => Ok(view.clone()),

            Self::Dynamic {
                cluster_view,
                trusted_operators,
            } => select_open_connection(cluster_view, trusted_operators)
                .await
                .cluster_view()
                .await
                .map_err(transport_err),
        }
    }

    async fn events(&self) -> ReadResult<impl Stream<Item = ReadResult<Event>> + Send + use<D>> {
        match self {
            Self::Static(_) => Ok(Either::Left(stream::pending())),

            Self::Dynamic {
                cluster_view,
                trusted_operators,
            } => {
                let stream = select_open_connection(cluster_view, trusted_operators)
                    .await
                    .events()
                    .await
                    .map_err(transport_err)?
                    .map_err(transport_err);

                Ok(Either::Right(stream))
            }
        }
    }
}

#[inline]
fn transport_err(err: impl ToString) -> ReadError {
    ReadError::Transport(err.to_string())
}

pub(crate) type Cluster<D> = wcn_cluster::Cluster<Config<D>>;
pub(crate) type View<D> = wcn_cluster::View<Config<D>>;

pub(crate) async fn update_task<D>(
    shutdown_rx: oneshot::Receiver<()>,
    cluster: Cluster<D>,
    view: Arc<ArcSwap<View<D>>>,
) where
    D: NodeData,
{
    let cluster_update_fut = async {
        let mut updates = cluster.updates();

        loop {
            view.store(cluster.view());

            if updates.next().await.is_none() {
                break;
            }
        }
    };

    tokio::select! {
        _ = cluster_update_fut => {},
        _ = shutdown_rx => {},
    };
}

pub(crate) async fn fetch_cluster_view(
    client: &ClusterClient,
    nodes: &[PeerAddr],
) -> Result<ClusterView, Error> {
    let num_nodes = nodes.len();
    let offset = rand::random_range(0..num_nodes);

    for idx in 0..num_nodes {
        let idx = (idx + offset) % num_nodes;

        match try_fetch_cluster_view(client, &nodes[idx]).await {
            Ok(view) => return Ok(view),

            Err(err) => {
                tracing::warn!(?err, "failed to fetch cluster view");
            }
        }
    }

    Err(Error::NoAvailableNodes)
}

async fn try_fetch_cluster_view(
    client: &ClusterClient,
    peer_addr: &PeerAddr,
) -> Result<ClusterView, Error> {
    let view = client
        .connect(peer_addr.addr, &peer_addr.id, (), "initial_cluster_view")
        .await
        .map_err(Error::internal)?
        .cluster_view()
        .await?;

    Ok(view)
}
