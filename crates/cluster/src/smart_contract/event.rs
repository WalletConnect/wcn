use {
    super::{AccountAddress, Keyspace, NodeOperator, Settings},
    crate::{self as cluster, migration, node_operator},
    derive_more::From,
    serde::{Deserialize, Serialize},
};

#[allow(unused_imports)] // for doc comments
use super::{Maintenance, Migration};

/// Events happening within a WCN Cluster.
#[derive(Clone, Debug, From, Serialize, Deserialize)]
pub enum Event {
    /// [`Migration`] has started.
    MigrationStarted(MigrationStarted),

    /// [`NodeOperator`] has completed the data pull.
    MigrationDataPullCompleted(MigrationDataPullCompleted),

    /// [`Migration`] has been completed.
    MigrationCompleted(MigrationCompleted),

    /// [`Migration`] has been aborted.
    MigrationAborted(MigrationAborted),

    /// [`Maintenance`] has started.
    MaintenanceStarted(MaintenanceStarted),

    /// [`Maintenance`] has been finished.
    MaintenanceFinished(MaintenanceFinished),

    /// [`NodeOperator`] has been added.
    NodeOperatorAdded(NodeOperatorAdded),

    /// [`NodeOperator`] has been updated.
    NodeOperatorUpdated(NodeOperatorUpdated),

    /// [`NodeOperator`] has been removed.
    NodeOperatorRemoved(NodeOperatorRemoved),

    /// [`Settings`] have been updated.
    SettingsUpdated(SettingsUpdated),
}

impl Event {
    pub(crate) fn cluster_version(&self) -> cluster::Version {
        match self {
            Event::MigrationStarted(evt) => evt.cluster_version,
            Event::MigrationDataPullCompleted(evt) => evt.cluster_version,
            Event::MigrationCompleted(evt) => evt.cluster_version,
            Event::MigrationAborted(evt) => evt.cluster_version,
            Event::MaintenanceStarted(evt) => evt.cluster_version,
            Event::MaintenanceFinished(evt) => evt.cluster_version,
            Event::NodeOperatorAdded(evt) => evt.cluster_version,
            Event::NodeOperatorUpdated(evt) => evt.cluster_version,
            Event::NodeOperatorRemoved(evt) => evt.cluster_version,
            Event::SettingsUpdated(evt) => evt.cluster_version,
        }
    }
}

/// [`Migration`] has started.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationStarted {
    /// ID of the [`Migration`] being started.
    pub migration_id: migration::Id,

    /// New [`Keyspace`] to migrate to.
    pub new_keyspace: Keyspace,

    /// UNIX timestamp (secs) when the [`Migration`] has started.
    pub at: u64,

    /// Updated [`cluster::Version`].
    pub cluster_version: cluster::Version,
}

/// [`NodeOperator`] has completed the data pull.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationDataPullCompleted {
    /// ID of the [`Migration`].
    pub migration_id: migration::Id,

    /// ID of the [`node_operator`] that completed the pull.
    pub operator_id: node_operator::Id,

    /// Updated [`cluster::Version`].
    pub cluster_version: cluster::Version,
}

/// [`Migration`] has been completed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationCompleted {
    /// ID of the completed [`Migration`].
    pub migration_id: migration::Id,

    /// ID of the [`node_operator`] that completed the last data pull.
    pub operator_id: node_operator::Id,

    /// Updated [`cluster::View`].
    pub cluster_version: cluster::Version,
}

/// [`Migration`] has been aborted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationAborted {
    /// ID of the [`Migration`].
    pub migration_id: migration::Id,

    /// UNIX timestamp (secs) when the [`Migration`] has been aborted.
    pub at: u64,

    /// Updated [`cluster::Version`].
    pub cluster_version: cluster::Version,
}

/// [`Maintenance`] has started.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaintenanceStarted {
    /// [`AccountAddress`] of the account that started the [`Maintenance`].
    pub by: AccountAddress,

    /// Updated [`ClusterVersion`].
    pub cluster_version: cluster::Version,
}

/// [`Maintenance`] has been finished.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaintenanceFinished {
    /// Updated [`ClusterVersion`].
    pub cluster_version: cluster::Version,
}

/// Event of a new [`NodeOperator`] being added to a WCN cluster.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeOperatorAdded {
    /// [`node_operator::Idx`] of the added [`NodeOperator`].
    pub idx: node_operator::Idx,

    /// [`NodeOperator`] being added.
    pub operator: NodeOperator,

    /// Updated [`cluster::Version`].
    pub cluster_version: cluster::Version,
}

/// Event of a [`NodeOperator`] being updated.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeOperatorUpdated {
    /// Updated [`NodeOperator`].
    pub operator: NodeOperator,

    /// Updated [`cluster::Version`].
    pub cluster_version: cluster::Version,
}

/// Event of a [`NodeOperator`] being removed from a WCN cluster.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeOperatorRemoved {
    /// [`Id`] of the [`NodeOperator`] being removed.
    pub id: node_operator::Id,

    /// Updated [`cluster::Version`].
    pub cluster_version: cluster::Version,
}

/// Event of [`Settings`] being updated.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingsUpdated {
    /// Updated [`Settings`].
    pub settings: Settings,

    /// Updated [`cluster::Version`].
    pub cluster_version: cluster::Version,
}
