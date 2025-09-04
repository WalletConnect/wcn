use {
    crate::{Error, NodeData, PeerAddr},
    arc_swap::ArcSwap,
    derive_more::derive::AsRef,
    derive_where::derive_where,
    futures::{Stream, StreamExt as _, TryStreamExt as _, future::Either, stream},
    std::{marker::PhantomData, sync::Arc, time::Duration},
    tokio::sync::oneshot,
    wc::future::FutureExt,
    wcn_cluster::{
        EncryptionKey,
        PeerId,
        node_operator,
        smart_contract::{ReadError, ReadResult},
    },
    wcn_cluster_api::{
        Address,
        ClusterApi,
        ClusterView,
        Read,
        rpc::client::{Cluster as ClusterClient, ClusterConnection},
    },
    wcn_storage_api::rpc::client::{Coordinator as CoordinatorClient, CoordinatorConnection},
};

#[derive(Clone)]
pub struct Node<D> {
    pub operator_id: node_operator::Id,
    pub node: wcn_cluster::Node,
    pub cluster_conn: ClusterConnection,
    pub coordinator_conn: CoordinatorConnection,
    pub data: D,
}

impl<D> AsRef<PeerId> for Node<D> {
    fn as_ref(&self) -> &PeerId {
        &self.node.peer_id
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
            data: D::init(&operator_id, &node),
            operator_id,
            node,
            cluster_conn,
            coordinator_conn,
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
            .with_timeout(Duration::from_millis(1500))
            .await
            .is_ok();

        if success {
            return conn;
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
    Dynamic(Arc<ArcSwap<View<D>>>),
}

#[derive(Debug, thiserror::Error)]
#[error("Method is not available")]
struct MethodNotAvailable;

impl<D> Read for SmartContract<D>
where
    D: NodeData,
{
    fn address(&self) -> ReadResult<Address> {
        Err(ReadError::Other(MethodNotAvailable.to_string()))
    }

    async fn cluster_view(&self) -> ReadResult<ClusterView> {
        match self {
            Self::Static(view) => Ok(view.clone()),

            Self::Dynamic(cluster) => select_open_connection(cluster)
                .await
                .cluster_view()
                .await
                .map_err(transport_err),
        }
    }

    async fn events(
        &self,
    ) -> ReadResult<impl Stream<Item = ReadResult<wcn_cluster::Event>> + Send + use<D>> {
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
