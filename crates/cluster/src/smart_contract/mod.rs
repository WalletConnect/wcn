//! Smart-contract managing the state of a WCN cluster.

pub use event::Event;
use {
    crate::{migration, node_operator},
    alloy::transports::http::reqwest,
    derive_more::From,
    derive_where::derive_where,
    futures::Stream,
    serde::{Deserialize, Serialize},
    std::{collections::HashSet, fmt, future::Future, str::FromStr},
};

pub mod event;
pub mod evm;
#[cfg(feature = "testing")]
pub mod testing;

#[allow(unused_imports)] // for doc comments
use crate::{keyspace, keyspace::MAX_OPERATORS, maintenance, settings, Cluster};

/// On-chain representation of [`crate::View`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterView {
    pub owner: AccountAddress,
    pub settings: Settings,

    pub node_operators: Vec<Option<NodeOperator>>,

    pub keyspaces: [Keyspace; 2],
    pub keyspace_version: u64,

    pub migration: Migration,
    pub maintenance: Maintenance,

    pub cluster_version: u128,
}

/// On-chain representation of [`crate::Settings`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub max_node_operator_data_bytes: u16,
    pub extra: Vec<u8>,
}

/// On-chain representation of [`crate::NodeOperator`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeOperator {
    pub id: node_operator::Id,
    pub data: Vec<u8>,
}

/// On-chain representation of [`crate::Keyspace`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Keyspace {
    pub operators: HashSet<node_operator::Idx>,
    pub replication_strategy: u8,
}

/// On-chain representation of [`crate::Migration`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    pub id: u64,
    pub pulling_operators: HashSet<node_operator::Idx>,
    pub started_at: u64,
    pub aborted_at: Option<u64>,
}

/// On-chain representation of [`crate::Maintenance`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Maintenance {
    pub slot: Option<AccountAddress>,
}

/// Deployer of WCN Cluster [`SmartContract`]s.
pub trait Deployer<SC> {
    /// Deploys a new [`SmartContract`].
    fn deploy(
        &self,
        initial_settings: Settings,
        initial_operators: Vec<NodeOperator>,
    ) -> impl Future<Output = Result<SC, DeploymentError>>;
}

/// Connector to WCN Cluster [`SmartContract`]s.
pub trait Connector<SC> {
    /// Connects to an existing [`SmartContract`].
    fn connect(&self, address: Address) -> impl Future<Output = Result<SC, ConnectionError>>;
}

/// Smart-contract managing the state of a WCN cluster.
pub trait SmartContract: Read + Write {}

/// Write [`SmartContract`] calls.
///
/// Logic invariants documented on the methods of this trait MUST be
/// implemented inside the on-chain implementation of the smart-contract itself.
pub trait Write {
    /// Returns the [`AccountAddress`] of the signer being used to sign
    /// transactions.
    fn signer(&self) -> Option<&AccountAddress>;

    /// Starts a new data [`migration`] process using the provided
    /// [`migration::Plan`].
    ///
    /// The implementation MUST validate the following invariants:
    /// - there's no ongoing data migration
    /// - there's no ongoing maintenance
    /// - all of the [`NodeOperator`]s within the provided [`Keyspace`] exist
    /// - the provided [`Keyspace`] has enough [`NodeOperator`]s (at least
    ///   [`keyspace::REPLICATION_FACTOR`])
    /// - the provided [`Keyspace`] differs from the current one
    /// - [`signer`](SmartContract::signer) is the owner of the
    ///   [`SmartContract`]
    ///
    /// The implementation MUST emit [`migration::Started`] event on success.
    fn start_migration(&self, new_keyspace: Keyspace) -> impl Future<Output = WriteResult<()>>;

    /// Marks that the [`signer`](SmartContract::signer) has completed the data
    /// pull required for completion of the current [`migration`].
    ///
    /// The implementation MUST validate the following invariants:
    /// - there's an ongoing data migration
    /// - the provided [`migration::Id`] matches the ID of the migration
    /// - [`signer`](Smart::signer) is a [`NodeOperator`] and is still pulling
    ///   the data
    ///
    /// If this [`NodeOperator`] is the last remaining one left to complete
    /// the data pull then the migration MUST be completed and
    /// [`migration::Completed`] event MUST be emitted.
    /// Otherwise the data pull MUST be marked as completed for the
    /// [`node::Operator`] and [`migration::DataPullCompleted`] MUST be emitted.
    fn complete_migration(&self, id: migration::Id)
        -> impl Future<Output = WriteResult<()>> + Send;

    /// Aborts the ongoing data [`migration`] process restoring the WCN cluster
    /// to the original state it had before the migration had started.
    ///
    /// The implementation MUST validate the following invariants:
    /// - there's an ongoing data migration
    /// - [`signer`](SmartContract::signer) is the owner of the
    ///   [`SmartContract`]
    ///
    /// The implementation MUST emit [`migration::Aborted`] event on success.
    fn abort_migration(&self, id: migration::Id) -> impl Future<Output = WriteResult<()>>;

    /// Starts a [`Maintenance`] process with the WCN Cluster.
    ///
    /// The implementation MUST validate the following invariants:
    /// - there's no ongoing data migration
    /// - there's no ongoing maintenance
    /// - [`signer`](SmartContract::signer) is either a [`NodeOperator`] under
    ///   the owner of the [`SmartContract`]
    ///
    /// The implementation MUST emit [`maintenance::Started`] event on success.
    fn start_maintenance(&self) -> impl Future<Output = WriteResult<()>>;

    /// Completes the ongoing [`maintenance`] process.
    ///
    /// The implementation MUST validate the following invariants:
    /// - there's an ongoing maintenance
    /// - [`signer`](SmartContract::signer) is either the [`NodeOperator`] that
    ///   [started](Manager::start_maintenance) the maintenance or the owner of
    ///   the [`SmartContract`]
    ///
    /// The implementation MUST emit [`maintenance::Finished`] event on
    /// success.
    fn finish_maintenance(&self) -> impl Future<Output = WriteResult<()>>;

    /// Adds a new [`NodeOperator`] to the WCN [`Cluster`].
    ///
    /// The implementation MUST validate the following invariants:
    /// - [`NodeOperator`] is not yet a member of the [`Cluster`]
    /// - [`MAX_OPERATORS`] limit is not reached
    /// - [`signer`](SmartContract::signer) is the owner of the
    ///   [`SmartContract`]
    ///
    /// The implementation MUST emit [`node_operator::Added`] event on
    /// success.
    fn add_node_operator(&self, operator: NodeOperator) -> impl Future<Output = WriteResult<()>>;

    /// Updates on-chain data of a [`node::Operator`].
    ///
    /// The implementation MUST validate the following invariants:
    /// - [`NodeOperator`] is a member of the [`Cluster`]
    /// - [`signer`](SmartContract::signer) is either the [`NodeOperator`] being
    ///   updated or the owner of the [`SmartContract`]
    ///
    /// The implementation MUST emit [`node_operator::Updated`] event on
    /// success.
    fn update_node_operator(&self, operator: NodeOperator)
        -> impl Future<Output = WriteResult<()>>;

    /// Removes a [`NodeOperator`] from the [`Cluster`].
    ///
    /// The implementation MUST validate the following invariants:
    /// - [`NodeOperator`] is a member of the [`Cluster`]
    /// - [`NodeOperator`] is not a member of either primary or [`Migration`]
    ///   [`Keyspace`]
    /// - [`signer`](SmartContract::signer) is the owner of the
    ///   [`SmartContract`]
    ///
    /// The implementation MUST emit [`node_operator::Removed`] event on
    /// success.
    fn remove_node_operator(&self, id: node_operator::Id) -> impl Future<Output = WriteResult<()>>;

    /// Updates [`Settings`] of the [`Cluster`].
    ///
    /// The implementation MUST validate the following invariants:
    /// - [`signer`](SmartContract::signer) is the owner of the
    ///   [`SmartContract`]
    ///
    /// The implementation MUST emit [`settings::Updated`] event on
    /// success.
    fn update_settings(&self, new_settings: Settings) -> impl Future<Output = WriteResult<()>>;
}

/// Read [`SmartContract`] calls.
pub trait Read: Sized + Send + Sync + 'static {
    /// Returns the current [`cluster::View`].
    fn cluster_view(&self) -> impl Future<Output = ReadResult<ClusterView>> + Send;

    /// Subscribes to WCN Cluster [`Events`].
    fn events(
        &self,
    ) -> impl Future<
        Output = ReadResult<impl Stream<Item = ReadResult<Event>> + Send + 'static + use<Self>>,
    > + Send;
}

/// [`SmartContract`] address.
pub type Address = AccountAddress;

/// Account address on the chain hosting WCN cluster [`SmartContract`].
#[derive(Clone, Copy, From, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountAddress([u8; 20]);

impl fmt::Debug for AccountAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: prexied with `0x` as per convention for Ethereum addresses
        write!(f, "0x{}", const_hex::encode(self.0))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: prexied with `0x` as per convention for Ethereum addresses
        write!(f, "0x{}", const_hex::encode(self.0))
    }
}

/// Error of [`Deployer::deploy`].
#[derive(Debug, thiserror::Error)]
#[error("{_0}")]
pub struct DeploymentError(String);

/// Error of [`Connector::connect`].
#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("Smart-contract with the provided address doesn't exist")]
    UnknownContract,

    #[error("Smart-contract with the provided address is not a WCN cluster")]
    WrongContract,

    #[error("Other: {0}")]
    Other(String),
}

/// [`Write`] error.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("Transport: {0}")]
    Transport(String),

    #[error("Transaction reverted: {0}")]
    Revert(String),

    #[error("Other: {0}")]
    Other(String),
}

/// [`Read`] error.
#[derive(Debug, thiserror::Error)]

pub enum ReadError {
    #[error("Transport: {0}")]
    Transport(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("Other: {0}")]
    Other(String),
}

/// [`Write`] result.
pub type WriteResult<T> = std::result::Result<T, WriteError>;

/// [`Read`] result.
pub type ReadResult<T> = std::result::Result<T, ReadError>;

#[derive(Debug, thiserror::Error)]
#[error("Invalid address: {0:?}")]
pub struct InvalidAddressError(String);

impl FromStr for AccountAddress {
    type Err = InvalidAddressError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        const_hex::decode_to_array(s)
            .map(Self)
            .map_err(|err| InvalidAddressError(err.to_string()))
    }
}

impl<SC> SmartContract for SC where SC: Read + Write {}
