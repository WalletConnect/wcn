//! Entity operating a set of nodes within a WCN cluster.

use {
    crate::{self as cluster, client, node, smart_contract, Client, Config, EncryptionKey, Node},
    derive_more::derive::AsRef,
    serde::{Deserialize, Serialize},
    std::sync::{
        atomic::{self, AtomicUsize},
        Arc,
    },
    tap::Tap,
};

/// Minimum number of [`Node`]s each [`NodeOperator`] is required to have.
pub const MIN_NODES: usize = 2;

/// Globally unique identifier of a [`NodeOperator`];
pub type Id = smart_contract::AccountAddress;

/// Locally unique identifier of a [`NodeOperator`] within a WCN cluster.
///
/// Refers to a position within the [`NodeOperators`] slot map.
pub type Idx = u8;

/// Name of a [`NodeOperator`].
///
/// Used for informational purposes only.
/// Expected to be unique within the cluster, but not enforced to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Name(String);

impl Name {
    /// Maximum allowed length of a [`Name`] (in bytes).
    pub const MAX_LENGTH: usize = 32;

    /// Tries to create a new [`Name`] out of the provided [`ToString`].
    ///
    /// Returns `None` if the string length exceeds [`Self::MAX_LENGTH`].
    pub fn new(s: impl ToString) -> Option<Self> {
        let s = s.to_string();
        (s.len() <= Self::MAX_LENGTH).then_some(Self(s))
    }

    /// Returns a reference to the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Entity operating a set of [`Node`]s within a WCN cluster.
#[derive(AsRef, Clone, Debug)]
pub struct NodeOperator<N = Node> {
    /// ID of this [`NodeOperator`].
    #[as_ref]
    pub id: Id,

    /// Name of the [`NodeOperator`].
    pub name: Name,

    /// List of [`Client`]s authorized to use the WCN cluster on behalf of the
    /// [`NodeOperator`].
    pub clients: Vec<Client>,

    /// List of [`Node`]s of the [`NodeOperator`].
    nodes: Vec<N>,

    // for load balancing
    counter: Arc<AtomicUsize>,
}

impl<N> NodeOperator<N> {
    /// Creates a new [`NodeOperator`].
    pub fn new(
        id: Id,
        name: Name,
        nodes: Vec<N>,
        clients: Vec<Client>,
    ) -> Result<Self, CreationError> {
        if nodes.len() < MIN_NODES {
            return Err(CreationError::TooFewNodes(nodes.len()));
        }

        Ok(Self {
            id,
            name,
            nodes,
            clients,
            // overflows and starts from `0`
            counter: Arc::new(usize::MAX.into()),
        })
    }

    /// Returns a [`Node`] of this [`NodeOperator`] responsible for the next
    /// request.
    ///
    /// [`Node`]s are being iterated in round-robin fashion for load-balancing
    /// purposes.
    pub fn next_node(&self) -> &N {
        // we've checked this in the constructor
        debug_assert!(!self.nodes.is_empty());

        let n = self.counter.fetch_add(1, atomic::Ordering::Relaxed);
        &self.nodes[n % self.nodes.len()]
    }

    /// Iterates over all of this [`NodeOperator`]'s [`Node`]s to find the one
    /// matching a predicate.
    ///
    /// This is similar to [`NodeOperator::next_node`] in using a counter for a
    /// round-robin load-balancing.
    pub fn find_next_node<F, R>(&self, cb: F) -> Option<R>
    where
        F: Fn(&N) -> Option<R>,
    {
        // we've checked this in the constructor
        debug_assert!(!self.nodes.is_empty());

        let num_nodes = self.nodes.len();
        let offset = self.counter.fetch_add(1, atomic::Ordering::Relaxed);

        for idx in 0..num_nodes {
            let idx = idx.wrapping_add(offset) % num_nodes;
            let res = cb(&self.nodes[idx]);

            if res.is_some() {
                return res;
            }
        }

        None
    }

    /// Returns [`Node`]s of this [`NodeOperator`].
    ///
    /// [`NodeOperator`] is guaranteed to always have at least 2 nodes.
    pub fn nodes(&self) -> &[N] {
        &self.nodes
    }

    /// Returns an [`Iterator`] of [`Node`]s of this [`NodeOperator`].
    ///
    /// Iterates over all [`Node`]s starting from an arbitrary position.
    /// Intended for load balancing purposes.
    ///
    /// [`NodeOperator`] is guaranteed to always have at least 2 nodes.
    pub fn nodes_lb_iter(&self) -> impl Iterator<Item = &N> {
        // TODO: this is suboptimal for > 2 nodes, because in case of a node failure the
        // next node in the list will receive all of the load of it's neighbor.
        //
        // Naive solution to this is to reallocate and shuffle. A better solution may be
        // to split the `Vec` on chunks of size <=N and then iterate over the chunks and
        // shuffle the chunks in-place.

        let n = self.counter.fetch_add(1, atomic::Ordering::Relaxed);
        let (left, right) = self.nodes.split_at(n % self.nodes.len());
        right.iter().chain(left.iter())
    }
}

impl NodeOperator {
    pub(super) fn try_into_sc(
        self,
        key: &EncryptionKey,
    ) -> Result<smart_contract::NodeOperator, TryIntoSmartContractError> {
        use TryIntoSmartContractError as Error;

        let nodes = self
            .nodes
            .into_iter()
            .map(|node| node.tap_mut(|n| n.encrypt(key)).into())
            .collect();

        let data = DataV0 {
            name: self.name,
            nodes,
            clients: self.clients.into_iter().map(Into::into).collect(),
        };

        let size = postcard::experimental::serialized_size(&data).map_err(Error::from_postcard)?;

        // reserve first byte for versioning
        let mut buf = vec![0; size + 1];
        buf[0] = 0; // current schema version
        postcard::to_slice(&data, &mut buf[1..]).map_err(Error::from_postcard)?;
        Ok(smart_contract::NodeOperator {
            id: self.id,
            data: buf,
        })
    }
}

impl<C: Config> NodeOperator<C> {
    pub(super) fn try_from_sc(
        operator: smart_contract::NodeOperator,
        cfg: &C,
    ) -> Result<NodeOperator<C::Node>, TryFromSmartContractError> {
        use TryFromSmartContractError as Error;

        let data_bytes = operator.data;

        if data_bytes.is_empty() {
            return Err(TryFromSmartContractError::EmptyBuffer);
        }

        let schema_version = data_bytes[0];
        let bytes = &data_bytes[1..];

        let data = match schema_version {
            0 => postcard::from_bytes::<DataV0>(bytes),
            ver => return Err(Error::UnknownSchemaVersion(ver)),
        }
        .map_err(Error::from_postcard)?;

        let nodes = data
            .nodes
            .into_iter()
            .map(|v0| Node::from(v0).tap_mut(|node| node.decrypt(cfg.as_ref())))
            .map(|node| cfg.new_node(operator.id, node))
            .collect();

        let clients = data.clients.into_iter().map(Into::into).collect();

        Ok(NodeOperator::new(operator.id, data.name, nodes, clients)?)
    }
}

// NOTE: The on-chain serialization is non self-describing!
// This `struct` can not be changed, a `struct` with a new schema version should
// be created instead.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct DataV0 {
    name: Name,
    nodes: Vec<node::V0>,
    clients: Vec<client::V0>,
}

#[derive(Debug, thiserror::Error)]
pub enum TryIntoSmartContractError {
    #[error("Codec: {0}")]
    Codec(String),
}

impl TryIntoSmartContractError {
    fn from_postcard(err: postcard::Error) -> Self {
        Self::Codec(format!("{err:?}"))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TryFromSmartContractError {
    #[error("Empty data buffer")]
    EmptyBuffer,

    #[error("Codec: {0}")]
    Codec(String),

    #[error("Unknown schema version: {0}")]
    UnknownSchemaVersion(u8),

    #[error(transparent)]
    Constructor(#[from] CreationError),
}

impl TryFromSmartContractError {
    fn from_postcard(err: postcard::Error) -> Self {
        Self::Codec(format!("{err:?}"))
    }
}

/// Validates that on-chain data size doesn't exceed
/// [`cluster::Settings::max_node_operator_data_bytes`].
pub(super) fn validate_data(
    data: &[u8],
    settings: &cluster::Settings,
) -> Result<(), DataTooLargeError> {
    let value = data.len();
    let limit = settings.max_node_operator_data_bytes as usize;

    if value > limit {
        return Err(DataTooLargeError { value, limit });
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum CreationError {
    #[error("Too few nodes: {_0} < {MIN_NODES}")]
    TooFewNodes(usize),
}

/// [`SerializedData`] size is too large.
#[derive(Debug, thiserror::Error)]
#[error("Node operator data size is too large (value: {value}, limit: {limit})")]
pub struct DataTooLargeError {
    value: usize,
    limit: usize,
}

#[derive(Debug, thiserror::Error)]
#[error("Node operator (id: {0}) is not a member of the cluster")]
pub struct NotFoundError(pub Id);

#[derive(Debug, thiserror::Error)]
#[error("Node operator (id: {0}) is already a member of the cluster")]
pub struct AlreadyExistsError(pub Id);
