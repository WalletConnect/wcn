use {
    crate::{
        keyspace::{self, ReplicationStrategy},
        node_operator,
        smart_contract,
        Keyspace,
    },
    futures::FutureExt as _,
    std::{collections::HashSet, mem, sync::Arc, time::Duration},
    time::OffsetDateTime,
};

/// At some point after [`Migration`] has started Replicas stop accepting
/// storage operations with the old [`keyspace::Version`].
///
/// We may naively assume that after this point it is safe to start serving
/// "pull data" requests.
/// However there may be in-flight storage operations, which
/// under a rare race condititon may be executed after the data pulling has
/// already started. Which then, in case if it's a write, may lead to this
/// write to be missed and the respective data to be lost during the data
/// transfer.
///
/// This additional time should mitigate this race condition.
pub(crate) const PULL_DATA_LEEWAY_TIME: Duration = Duration::from_secs(2);

/// Identifier of a [`Migration`].
pub type Id = u64;

/// Data migration process within a WCN cluster.
#[derive(Debug, Clone)]
pub struct Migration<S = ()> {
    pub(crate) id: Id,
    pub(crate) started_at: OffsetDateTime,
    pub(crate) state: State<S>,
}

#[derive(Debug, Default, Clone)]
pub(crate) enum State<Shards = ()> {
    Started {
        keyspace: Arc<Keyspace<Shards>>,
        pulling_operators: HashSet<node_operator::Idx>,
    },
    Aborted {
        at: OffsetDateTime,
    },
    #[default]
    Completed,
}

/// [`Migration`] plan.
pub struct Plan {
    /// Set of [`node_operator`]s to remove from the current [`Keyspace`].
    pub remove: HashSet<node_operator::Id>,

    /// Set of [`node_operator`]s to add to the current [`Keyspace`].
    pub add: HashSet<node_operator::Id>,

    /// New [`ReplicationStrategy`] to use.
    pub replication_strategy: ReplicationStrategy,
}

impl<S> Migration<S> {
    /// Returns [`Id`] of this [`Migration`].
    pub fn id(&self) -> Id {
        self.id
    }

    /// Returns the new [`Keyspace`] this [`Migration`] is migrating to (if in
    /// progress).
    pub(crate) fn keyspace(&self) -> Option<&Keyspace<S>> {
        match &self.state {
            State::Started { keyspace, .. } => Some(keyspace.as_ref()),
            State::Aborted { .. } | State::Completed => None,
        }
    }

    /// Indicates whether the specified [`node_operator`] is still in process of
    /// pulling the data.
    pub(crate) fn is_pulling(&self, idx: node_operator::Idx) -> bool {
        if let State::Started {
            pulling_operators, ..
        } = &self.state
        {
            pulling_operators.contains(&idx)
        } else {
            false
        }
    }

    pub(crate) fn is_in_progress(&self) -> bool {
        matches!(self.state, State::Started { .. })
    }

    pub(crate) fn pulling_count(&self) -> usize {
        if let State::Started {
            pulling_operators, ..
        } = &self.state
        {
            pulling_operators.len()
        } else {
            0
        }
    }

    pub(crate) fn complete_pull(&mut self, idx: node_operator::Idx) {
        if let State::Started {
            pulling_operators, ..
        } = &mut self.state
        {
            pulling_operators.remove(&idx);
        }
    }

    pub(crate) fn complete(&mut self) -> Option<Arc<Keyspace<S>>> {
        if let State::Started { keyspace, .. } = mem::take(&mut self.state) {
            Some(keyspace)
        } else {
            None
        }
    }

    pub(crate) fn require_id(&self, id: Id) -> Result<&Migration<S>, WrongIdError> {
        if id != self.id {
            return Err(WrongIdError(id, self.id));
        }

        Ok(self)
    }

    pub(crate) fn require_pulling(
        &self,
        idx: node_operator::Idx,
    ) -> Result<&Self, OperatorNotPullingError> {
        if !self.is_pulling(idx) {
            return Err(OperatorNotPullingError(idx));
        }

        Ok(self)
    }

    pub(crate) fn require_pulling_count(
        &self,
        expected: usize,
    ) -> Result<&Self, WrongPullingOperatorsCountError> {
        let count = self.pulling_count();
        if count != expected {
            return Err(WrongPullingOperatorsCountError(expected, count));
        }

        Ok(self)
    }
}

impl Migration {
    pub(crate) fn try_from_sc(
        migration: smart_contract::Migration,
        keyspace: smart_contract::Keyspace,
    ) -> Result<Self, TryFromSmartContractError> {
        let state = if let Some(at) = migration.aborted_at {
            State::Aborted {
                at: parse_timestamp(at)?,
            }
        } else if migration.pulling_operators.is_empty() {
            State::Completed
        } else {
            State::Started {
                keyspace: Arc::new(keyspace.try_into()?),
                pulling_operators: migration.pulling_operators.clone(),
            }
        };

        Ok(Migration {
            id: migration.id,
            started_at: parse_timestamp(migration.started_at)?,
            state,
        })
    }

    pub(crate) async fn calculate_keyspace<Shards>(self) -> Migration<Shards>
    where
        Keyspace: keyspace::sealed::Calculate<Shards>,
    {
        Migration {
            id: self.id,
            started_at: self.started_at,
            state: match self.state {
                State::Started {
                    keyspace,
                    pulling_operators,
                } => State::Started {
                    keyspace: keyspace::sealed::Calculate::<Shards>::calculate_shards(
                        Arc::unwrap_or_clone(keyspace),
                    )
                    .map(Arc::new)
                    .await,

                    pulling_operators,
                },
                State::Aborted { at } => State::Aborted { at },
                State::Completed => State::Completed,
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Migration(id: {_0}) in progress")]
pub struct InProgressError(pub Id);

#[derive(Debug, thiserror::Error)]
#[error("No migration")]
pub struct NotFoundError;

#[derive(Debug, thiserror::Error)]
#[error("Wrong migration ID: {_0} != {_1}")]
pub struct WrongIdError(pub Id, pub Id);

#[derive(Debug, thiserror::Error)]
#[error("NodeOperator(idx: {_0}) is not currently pulling data")]
pub struct OperatorNotPullingError(pub node_operator::Idx);

#[derive(Debug, thiserror::Error)]
#[error("Wrong pulling operators count: {_0} != {_1}")]
pub struct WrongPullingOperatorsCountError(pub usize, pub usize);

#[derive(Debug, thiserror::Error)]
pub enum TryFromSmartContractError {
    #[error(transparent)]
    Keyspace(#[from] keyspace::TryFromSmartContractError),

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(u64),
}

pub(super) fn parse_timestamp(ts: u64) -> Result<OffsetDateTime, TryFromSmartContractError> {
    i64::try_from(ts)
        .ok()
        .and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok())
        .ok_or_else(|| TryFromSmartContractError::InvalidTimestamp(ts))
}
