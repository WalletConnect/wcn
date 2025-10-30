use {
    crate::{Connector, Error, NodeData, PeerAddr},
    arc_swap::ArcSwap,
    derive_more::derive::AsRef,
    derive_where::derive_where,
    futures::{FutureExt as _, Stream, StreamExt as _, TryStreamExt as _, future::Either, stream},
    futures_concurrency::future::Race as _,
    monitoring::{ConnectionState, NodeState},
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
        metrics::{self, StringLabel},
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
    wcn_storage_api::{
        Namespace,
        rpc::{
            CoordinatorApi,
            client::{Coordinator as CoordinatorClient, CoordinatorConnection},
        },
    },
};

mod monitoring;

#[derive(Clone)]
pub struct Node<D> {
    pub peer_id: PeerId,
    pub public_cluster_conn: ClusterConnection,
    pub public_coordinator_conn: CoordinatorConnection,
    pub private_cluster_conn: Option<ClusterConnection>,
    pub private_coordinator_conn: Option<CoordinatorConnection>,
    pub data: D,
    state: Arc<NodeState>,
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
    authorized_namespace: Namespace,
    _marker: PhantomData<D>,
}

impl<D> Config<D> {
    pub fn new(
        encryption_key: EncryptionKey,
        cluster_api: ClusterClient,
        coordinator_api: CoordinatorClient,
        authorized_namespace: Namespace,
    ) -> Self {
        Self {
            encryption_key,
            cluster_api,
            coordinator_api,
            authorized_namespace,
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

        let public_cluster_conn = self.cluster_api.new_connection(public_addr, id, ());
        let public_coordinator_conn = self.coordinator_api.new_connection(public_addr, id, ());

        let (private_cluster_conn, private_coordinator_conn) = node
            .primary_socket_addr_private()
            .map(|addr| {
                let cluster_conn = self.cluster_api.new_connection(addr, id, ());
                let coordinator_conn = self.coordinator_api.new_connection(addr, id, ());

                (cluster_conn, coordinator_conn)
            })
            .unzip();

        let state = Arc::new(NodeState::new(
            public_coordinator_conn.clone(),
            private_coordinator_conn.clone(),
            self.authorized_namespace,
        ));

        Node {
            peer_id: node.peer_id,
            public_cluster_conn,
            public_coordinator_conn,
            private_cluster_conn,
            private_coordinator_conn,
            data: D::new(&operator_id, &node),
            state,
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

    let metrics_fut = async {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;

            view.load().node_operators().find_next_operator(|op| {
                op.find_next_node(|node| {
                    let operator_name = op.name.as_str();

                    let conn_metrics = |state: &ConnectionState| {
                        let remote_addr = state.remote_addr();
                        let peer_id = &node.peer_id;

                        metrics::histogram!(
                            "wcn_connection_latency",
                            StringLabel<"operator"> => operator_name,
                            StringLabel<"peer_id", PeerId> => peer_id,
                            StringLabel<"remote_addr", SocketAddrV4> => remote_addr
                        )
                        .record(state.latency());

                        metrics::gauge!(
                            "wcn_connection_suspicion_score",
                            StringLabel<"operator"> => operator_name,
                            StringLabel<"peer_id", PeerId> => peer_id,
                            StringLabel<"remote_addr", SocketAddrV4> => remote_addr
                        )
                        .set(state.suspicion_score());
                    };

                    let state = node.state.as_ref();

                    conn_metrics(state.public_state());
                    state.private_state().map(conn_metrics);

                    None::<()>
                })
            });
        }
    };

    let shutdown_fut = shutdown_rx.map(|_| ());

    (cluster_update_fut, metrics_fut, shutdown_fut).race().await;
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
        .connect(peer_addr.addr, &peer_addr.id, ())
        .await
        .map_err(Error::internal)?
        .cluster_view()
        .await?;

    Ok(view)
}
