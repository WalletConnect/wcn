use {
    crate::{Error, NodeData, PeerAddr},
    arc_swap::ArcSwap,
    derive_more::derive::AsRef,
    derive_where::derive_where,
    futures::{Stream, StreamExt as _, TryStreamExt as _, future::Either, stream},
    std::{
        marker::PhantomData,
        sync::{
            Arc,
            atomic::{self, AtomicUsize},
        },
        time::Duration,
    },
    tokio::sync::oneshot,
    wc::future::FutureExt,
    wcn_cluster::{
        EncryptionKey,
        PeerId,
        node_operator,
        smart_contract::{ReadError, ReadResult},
    },
    wcn_cluster_api::{
        ClusterApi,
        ClusterView,
        Event,
        Read,
        rpc::client::{Cluster as ClusterClient, ClusterConnection},
    },
    wcn_storage_api::rpc::client::{Coordinator as CoordinatorClient, CoordinatorConnection},
};

const CONNECTION_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Clone)]
pub struct Node<D> {
    pub peer_id: PeerId,
    pub cluster_conn: ClusterConnection,
    pub coordinator_conn: CoordinatorConnection,
    pub data: D,
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
        let cluster_conn =
            self.cluster_api
                .new_connection(node.primary_socket_addr(), &node.peer_id, ());
        let coordinator_conn =
            self.coordinator_api
                .new_connection(node.primary_socket_addr(), &node.peer_id, ());

        Node {
            peer_id: node.peer_id,
            cluster_conn,
            coordinator_conn,
            data: D::new(&operator_id, &node),
        }
    }
}

async fn select_open_connection<D>(cluster: &ArcSwap<View<D>>) -> ClusterConnection
where
    D: NodeData,
{
    loop {
        let conn = cluster
            .load()
            .node_operators()
            .next()
            .next_node()
            .cluster_conn
            .clone();

        let success = conn
            .wait_open()
            .with_timeout(CONNECTION_TIMEOUT)
            .await
            .is_ok();

        if success {
            return conn;
        }
    }
}

pub(crate) struct FixedNodeList {
    nodes: Vec<ClusterConnection>,
    idx: AtomicUsize,
}

impl FixedNodeList {
    fn new(client: &ClusterClient, nodes: &[PeerAddr]) -> Result<Self, Error> {
        if nodes.is_empty() {
            return Err(Error::NoAvailableNodes);
        }

        let nodes = nodes
            .iter()
            .map(|addr| client.new_connection(addr.addr, &addr.id, ()))
            .collect();

        Ok(Self {
            nodes,
            idx: Default::default(),
        })
    }

    async fn next(&self) -> ClusterConnection {
        loop {
            let idx = self.idx.fetch_add(1, atomic::Ordering::Relaxed) % self.nodes.len();
            let conn = self.nodes[idx].clone();

            let success = conn
                .wait_open()
                .with_timeout(CONNECTION_TIMEOUT)
                .await
                .is_ok();

            if success {
                return conn;
            }
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
#[allow(dead_code)]
pub(crate) enum SmartContract<D: NodeData> {
    Static(ClusterView),
    Dynamic(Arc<ArcSwap<View<D>>>),
    FixedNodeList(FixedNodeList),
}

impl<D: NodeData> SmartContract<D> {
    pub(crate) fn fixed_node_list(
        client: &ClusterClient,
        nodes: &[PeerAddr],
    ) -> Result<Self, Error> {
        FixedNodeList::new(client, nodes).map(Self::FixedNodeList)
    }
}

impl<D: NodeData> Read for SmartContract<D> {
    async fn cluster_view(&self) -> ReadResult<ClusterView> {
        match self {
            Self::Static(view) => Ok(view.clone()),

            Self::Dynamic(cluster) => select_open_connection(cluster)
                .await
                .cluster_view()
                .await
                .map_err(transport_err),

            Self::FixedNodeList(nodes) => nodes
                .next()
                .await
                .cluster_view()
                .await
                .map_err(transport_err),
        }
    }

    async fn events(&self) -> ReadResult<impl Stream<Item = ReadResult<Event>> + Send + use<D>> {
        match self {
            Self::Static(_) => Ok(Either::Left(stream::pending())),

            Self::Dynamic(cluster) => {
                let stream = select_open_connection(cluster)
                    .await
                    .events()
                    .await
                    .map_err(transport_err)?
                    .map_err(transport_err);

                Ok(Either::Right(stream))
            }

            Self::FixedNodeList(nodes) => {
                let stream = nodes
                    .next()
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
        .connect(peer_addr.addr, &peer_addr.id, ())
        .await
        .map_err(Error::internal)?
        .cluster_view()
        .await?;

    Ok(view)
}
