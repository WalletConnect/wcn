//! Read-only view of a WCN cluster.

use {
    crate::{
        self as cluster,
        keyspace,
        maintenance,
        migration,
        node_operator,
        node_operators,
        settings,
        smart_contract,
        Config,
        Keyspace,
        Maintenance,
        Migration,
        NodeOperator,
        NodeOperators,
        Ownership,
        Settings,
    },
    derive_where::derive_where,
    std::sync::Arc,
    tap::Pipe as _,
    time::OffsetDateTime,
};

/// Read-only view of a WCN cluster.
#[derive_where(Clone)]
pub struct View<C: Config> {
    pub(super) node_operators: NodeOperators<C::Node>,

    pub(super) ownership: Ownership,
    pub(super) settings: Settings,

    pub(super) keyspace: Arc<Keyspace<C::KeyspaceShards>>,
    pub(super) keyspace_version: keyspace::Version,

    pub(super) migration: Migration<C::KeyspaceShards>,

    pub(super) maintenance: Option<Maintenance>,

    pub(super) cluster_version: cluster::Version,
}

impl<C: Config<KeyspaceShards = keyspace::Shards>> View<C> {
    pub fn primary_replica_set(&self, key: u64) -> keyspace::ReplicaSet<&NodeOperator<C::Node>> {
        // NOTE(unwrap): we use `Keyspace::validate` every time it enters the system,
        // therefore guaranteeing that every `NodeOperatorIdx` is valid.
        self.keyspace
            .shard(key)
            .replica_set
            .map(|idx| self.node_operators.get_by_idx(idx).unwrap())
    }

    pub fn secondary_replica_set(
        &self,
        key: u64,
    ) -> Option<keyspace::ReplicaSet<&NodeOperator<C::Node>>> {
        if !self.is_secondary_keyspace_shadowing_on() {
            return None;
        }

        // NOTE(unwrap): we use `Keyspace::validate` every time it enters the system,
        // therefore guaranteeing that every `NodeOperatorIdx` is valid.
        self.migration
            .keyspace()?
            .shard(key)
            .replica_set
            .map(|idx| self.node_operators.get_by_idx(idx).unwrap())
            .pipe(Some)
    }

    /// Returns [`keyspace::Shard`]s of the primary [`Keyspace`].
    pub fn primary_keyspace_shards(
        &self,
    ) -> impl Iterator<Item = (keyspace::ShardId, keyspace::Shard<&NodeOperator<C::Node>>)> {
        // NOTE(unwrap): we use `Keyspace::validate` every time it enters the system,
        // therefore guaranteeing that every `NodeOperatorIdx` is valid.
        self.keyspace.shards().map(|(id, shard)| {
            (id, keyspace::Shard {
                replica_set: shard
                    .replica_set
                    .map(|idx| self.node_operators.get_by_idx(idx).unwrap()),
            })
        })
    }

    /// Returns [`keyspace::Shard`]s of the secondary [`Keyspace`].
    pub fn secondary_keyspace_shards(
        &self,
    ) -> Option<impl Iterator<Item = (keyspace::ShardId, keyspace::Shard<&NodeOperator<C::Node>>)>>
    {
        if !self.is_secondary_keyspace_shadowing_on() {
            return None;
        }

        // NOTE(unwrap): we use `Keyspace::validate` every time it enters the system,
        // therefore guaranteeing that every `NodeOperatorIdx` is valid.
        self.migration
            .keyspace()?
            .shards()
            .map(|(id, shard)| {
                (id, keyspace::Shard {
                    replica_set: shard
                        .replica_set
                        .map(|idx| self.node_operators.get_by_idx(idx).unwrap()),
                })
            })
            .pipe(Some)
    }

    /// Indicates whether the specified [`node_operator`] is still in process of
    /// pulling data as part of a data [`migration`] process.
    pub fn is_pulling(&self, operator_id: &node_operator::Id) -> bool {
        self.node_operators()
            .get_idx(operator_id)
            .map(|operator_idx| self.migration.is_pulling(operator_idx))
            .unwrap_or_default()
    }
}

impl<C: Config> View<C> {
    /// Returns [`Ownership`] of the WCN cluster.
    pub fn ownership(&self) -> &Ownership {
        &self.ownership
    }

    /// Returns [`Settings`] of the WCN cluster.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Returns the primary [`Keyspace`] of the WCN cluster.
    pub fn keyspace(&self) -> &Keyspace<C::KeyspaceShards> {
        &self.keyspace
    }

    /// Returns the ongoing [`Migration`] of the WCN cluster.
    pub fn migration(&self) -> Option<&Migration<C::KeyspaceShards>> {
        if self.migration.is_in_progress() {
            Some(&self.migration)
        } else {
            None
        }
    }

    /// Returns the ongoing [`Maintenance`] of the WCN cluster.
    pub fn maintenance(&self) -> Option<&Maintenance> {
        self.maintenance.as_ref()
    }

    /// Returns [`NodeOperators`] of the WCN cluster.
    pub fn node_operators(&self) -> &NodeOperators<C::Node> {
        &self.node_operators
    }

    /// Indicates whether storage writes should be shadowed into the secondary
    /// keyspace at this moment.
    pub(crate) fn is_secondary_keyspace_shadowing_on(&self) -> bool {
        if !self.migration.is_in_progress() {
            return false;
        }

        self.settings
            .has_event_propagated(self.migration.started_at)
    }

    /// Returns [`OffsetDateTime`] after which WCN Nodes are supposed to start
    /// pulling data during a [`Migration`] process.
    pub fn data_pull_scheduled_after(&self) -> Option<OffsetDateTime> {
        let migration = self.migration()?;

        let time = migration.started_at
            + self.settings.event_propagation_latency
            + self.settings.clock_skew
            + migration::PULL_DATA_LEEWAY_TIME;

        Some(time)
    }

    /// Returns the effective [`keyspace::Version`] of this [`Cluster`].
    pub fn keyspace_version(&self) -> keyspace::Version {
        let migration = &self.migration;
        let settings = self.settings();

        match migration.state {
            migration::State::Started { .. }
                if settings.has_event_propagated(migration.started_at) =>
            {
                self.keyspace_version
            }
            migration::State::Started { .. } => self.keyspace_version - 1,

            migration::State::Aborted { at } if settings.has_event_propagated(at) => {
                self.keyspace_version
            }
            migration::State::Aborted { .. } => self.keyspace_version + 1,

            migration::State::Completed => self.keyspace_version,
        }
    }

    /// Checks whether a storage operation with the specified
    /// [`keyspace::Version`] can be executed at this moment.
    pub fn validate_storage_operation(&self, keyspace_version: keyspace::Version) -> bool {
        let settings = self.settings();

        let ver = self.keyspace_version;

        let (old, new, transition_time) = match self.migration.state {
            migration::State::Started { .. } => (ver - 1, ver, self.migration.started_at),
            migration::State::Aborted { at } => (ver + 1, ver, at),
            migration::State::Completed => return keyspace_version == ver,
        };

        let now = OffsetDateTime::now_utc();
        let transition_time = settings.event_propagation_time(transition_time);
        let transition_time_frame = settings.clock_skew_time_frame(transition_time);

        if now < *transition_time_frame.start() {
            return keyspace_version == old;
        }

        if now <= *transition_time_frame.end() {
            return [old, new].contains(&keyspace_version);
        }

        keyspace_version == new
    }

    /// Checks whether a data pull with the specified [`keyspace::Version`] can
    /// be executed at this moment.
    pub fn validate_data_pull(&self, keyspace_version: keyspace::Version) -> bool {
        let Some(scheduled_after) = self.data_pull_scheduled_after() else {
            return false;
        };

        OffsetDateTime::now_utc() >= scheduled_after && self.keyspace_version == keyspace_version
    }

    pub(super) fn require_no_migration(&self) -> Result<(), migration::InProgressError> {
        if self.migration.is_in_progress() {
            return Err(migration::InProgressError(self.migration.id()));
        }

        Ok(())
    }

    pub(super) fn require_no_maintenance(&self) -> Result<(), maintenance::InProgressError> {
        if let Some(maintenance) = self.maintenance() {
            return Err(maintenance::InProgressError(*maintenance.slot()));
        }

        Ok(())
    }

    pub(super) fn require_migration(
        &self,
    ) -> Result<&Migration<C::KeyspaceShards>, migration::NotFoundError> {
        if !self.migration.is_in_progress() {
            return Err(migration::NotFoundError);
        }

        Ok(&self.migration)
    }

    pub(super) fn require_maintenance(&self) -> Result<&Maintenance, maintenance::NotFoundError> {
        self.maintenance.as_ref().ok_or(maintenance::NotFoundError)
    }

    /// Applies the provided [`smart_contract::Event`] to this [`View`].
    pub async fn apply_event(mut self, cfg: &C, event: smart_contract::Event) -> Result<Self, Error>
    where
        Keyspace: keyspace::sealed::Calculate<C::KeyspaceShards>,
    {
        use smart_contract::Event;

        let new_version = event.cluster_version();

        if new_version != self.cluster_version + 1 {
            return Err(Error::ClusterVersionNotMonotonic(
                self.cluster_version,
                new_version,
            ));
        }

        match event {
            Event::MigrationStarted(evt) => evt.apply(&mut self).await,
            Event::MigrationDataPullCompleted(evt) => evt.apply(&mut self),
            Event::MigrationCompleted(evt) => evt.apply(&mut self),
            Event::MigrationAborted(evt) => evt.apply(&mut self),
            Event::MaintenanceStarted(evt) => evt.apply(&mut self),
            Event::MaintenanceFinished(evt) => evt.apply(&mut self),
            Event::NodeOperatorAdded(evt) => evt.apply(cfg, &mut self),
            Event::NodeOperatorUpdated(evt) => evt.apply(cfg, &mut self),
            Event::NodeOperatorRemoved(evt) => evt.apply(&mut self),
            Event::SettingsUpdated(evt) => evt.apply(cfg, &mut self),
        }?;

        self.cluster_version = new_version;

        Ok(self)
    }
}

impl<C: Config> View<C> {
    pub(super) async fn try_from_sc(
        view: smart_contract::ClusterView,
        cfg: &C,
    ) -> Result<Self, TryFromSmartContractError>
    where
        Keyspace: keyspace::sealed::Calculate<C::KeyspaceShards>,
    {
        let node_operators = view
            .node_operators
            .into_iter()
            .map(|slot| slot.map(|operator| NodeOperator::from_sc(operator, cfg)))
            .pipe(NodeOperators::from_slots)?;

        let ownership = Ownership::new(view.owner);
        let settings = view.settings.try_into()?;
        cfg.update_settings(&settings);

        let is_migration_in_progress = !view.migration.pulling_operators.is_empty();

        let primary_keyspace_version = if is_migration_in_progress {
            view.keyspace_version
                .checked_sub(1)
                .ok_or(TryFromSmartContractError::InvalidKeyspaceVersion)?
        } else {
            view.keyspace_version
        };

        let [keyspace0, keyspace1] = view.keyspaces;

        let (primary_keyspace, secondary_keyspace) = if primary_keyspace_version % 2 == 0 {
            (keyspace0, keyspace1)
        } else {
            (keyspace1, keyspace0)
        };

        let keyspace: Keyspace = primary_keyspace.try_into()?;
        keyspace.validate(&node_operators)?;

        let migration = Migration::try_from_sc(view.migration, secondary_keyspace)?;
        if let Some(keyspace) = migration.keyspace() {
            keyspace.validate(&node_operators)?;
        }

        let (keyspace, migration) =
            tokio::join!(keyspace.calculate(), migration.calculate_keyspace());

        let maintenance = view.maintenance.slot.map(Maintenance::new);

        Ok(View {
            node_operators,
            ownership,
            settings,
            keyspace: Arc::new(keyspace),
            keyspace_version: view.keyspace_version,
            migration,
            maintenance,
            cluster_version: view.cluster_version,
        })
    }
}

impl smart_contract::event::MigrationStarted {
    async fn apply<C: Config>(self, view: &mut View<C>) -> Result<()>
    where
        Keyspace: keyspace::sealed::Calculate<C::KeyspaceShards>,
    {
        view.require_no_migration()?;
        view.require_no_maintenance()?;

        let new_keyspace: Keyspace = self.new_keyspace.try_into()?;
        new_keyspace.validate(view.node_operators())?;

        view.keyspace().require_diff(&new_keyspace)?;

        let new_keyspace = new_keyspace.calculate().await;

        view.migration = Migration {
            id: self.migration_id,
            started_at: migration::parse_timestamp(self.at)?,
            state: migration::State::Started {
                pulling_operators: new_keyspace.operators().collect(),
                keyspace: Arc::new(new_keyspace),
            },
        };

        view.keyspace_version += 1;

        Ok(())
    }
}

impl smart_contract::event::MigrationDataPullCompleted {
    pub(super) fn apply<A: Config>(self, view: &mut View<A>) -> Result<()> {
        let idx = view.node_operators.require_idx(&self.operator_id)?;
        view.require_migration()?
            .require_id(self.migration_id)?
            .require_pulling(idx)?;

        view.migration.complete_pull(idx);

        Ok(())
    }
}

impl smart_contract::event::MigrationCompleted {
    pub(super) fn apply<C: Config>(self, view: &mut View<C>) -> Result<()> {
        let idx = view.node_operators.require_idx(&self.operator_id)?;
        view.require_migration()?
            .require_id(self.migration_id)?
            .require_pulling(idx)?
            .require_pulling_count(1)?;

        // NOTE(unwrap): We just checked that migration exists.
        view.keyspace = view.migration.complete().unwrap();

        Ok(())
    }
}

impl smart_contract::event::MigrationAborted {
    pub(super) fn apply<C: Config>(self, view: &mut View<C>) -> Result<()> {
        view.require_migration()?.require_id(self.migration_id)?;
        view.migration.state = migration::State::Aborted {
            at: migration::parse_timestamp(self.at)?,
        };
        view.keyspace_version -= 1;

        Ok(())
    }
}

impl smart_contract::event::MaintenanceStarted {
    pub(super) fn apply<C: Config>(self, view: &mut View<C>) -> Result<()> {
        view.require_no_migration()?;
        view.require_no_maintenance()?;

        view.maintenance = Some(Maintenance::new(self.by));

        Ok(())
    }
}

impl smart_contract::event::MaintenanceFinished {
    pub(super) fn apply<C: Config>(self, view: &mut View<C>) -> Result<()> {
        view.require_maintenance()?;
        view.maintenance = None;

        Ok(())
    }
}

impl smart_contract::event::NodeOperatorAdded {
    pub(super) fn apply<C: Config>(self, cfg: &C, view: &mut View<C>) -> Result<()> {
        view.node_operators()
            .require_not_exists(&self.operator.id)?
            .require_free_slot(self.idx)?;

        view.node_operators
            .set(self.idx, Some(NodeOperator::from_sc(self.operator, cfg)));

        Ok(())
    }
}

impl smart_contract::event::NodeOperatorUpdated {
    pub(super) fn apply<C: Config>(self, cfg: &C, view: &mut View<C>) -> Result<()> {
        let idx = view.node_operators.require_idx(&self.operator.id)?;
        view.node_operators
            .set(idx, Some(NodeOperator::from_sc(self.operator, cfg)));

        Ok(())
    }
}

impl smart_contract::event::NodeOperatorRemoved {
    pub(super) fn apply<C: Config>(self, view: &mut View<C>) -> Result<()> {
        let idx = view.node_operators.require_idx(&self.id)?;
        view.node_operators.set(idx, None);

        Ok(())
    }
}

impl smart_contract::event::SettingsUpdated {
    pub(super) fn apply<C: Config>(self, cfg: &C, view: &mut View<C>) -> Result<()> {
        view.settings = self.settings.try_into()?;
        cfg.update_settings(&view.settings);

        Ok(())
    }
}

/// Error of [`View::apply_event`] caused by a race condition or by an incorrect
/// implementation of the [`SmartContract`](crate::SmartContract).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cluster version change wasn't monotonic: {_0} -> {_1}")]
    ClusterVersionNotMonotonic(cluster::Version, cluster::Version),

    #[error(transparent)]
    MigrationInProgress(#[from] migration::InProgressError),

    #[error(transparent)]
    MaintenanceInProgress(#[from] maintenance::InProgressError),

    #[error(transparent)]
    NoMigration(#[from] migration::NotFoundError),

    #[error(transparent)]
    NoMaintenance(#[from] maintenance::NotFoundError),

    #[error(transparent)]
    WrongMigrationId(#[from] migration::WrongIdError),

    #[error(transparent)]
    SameKeyspace(#[from] keyspace::SameKeyspaceError),

    #[error(transparent)]
    KeyspaceUnknownOperator(#[from] keyspace::UnknownNodeOperator),

    #[error(transparent)]
    NotPulling(#[from] migration::OperatorNotPullingError),

    #[error(transparent)]
    WrongPullingOperatorsCount(#[from] migration::WrongPullingOperatorsCountError),

    #[error(transparent)]
    OperatorNotFound(#[from] node_operator::NotFoundError),

    #[error(transparent)]
    OperatorAlreadyExists(#[from] node_operator::AlreadyExistsError),

    #[error(transparent)]
    OperatorSlotOccupied(#[from] node_operators::SlotOccupiedError),

    #[error(transparent)]
    InvalidNodeOperator(#[from] node_operator::TryFromSmartContractError),

    #[error(transparent)]
    InvalidSettings(#[from] settings::TryFromSmartContractError),

    #[error(transparent)]
    InvalidKeyspace(#[from] keyspace::TryFromSmartContractError),

    #[error(transparent)]
    InvalidMigration(#[from] migration::TryFromSmartContractError),
}

#[derive(Debug, thiserror::Error)]
pub enum TryFromSmartContractError {
    #[error("Invalid keyspace version")]
    InvalidKeyspaceVersion,

    #[error(transparent)]
    KeyspaceUnknownOperator(#[from] keyspace::UnknownNodeOperator),

    #[error(transparent)]
    NodeOperatorsCreation(#[from] node_operators::CreationError),

    #[error(transparent)]
    InvalidSettings(#[from] settings::TryFromSmartContractError),

    #[error(transparent)]
    InvalidNodeOperator(#[from] node_operator::TryFromSmartContractError),

    #[error(transparent)]
    InvalidKeyspace(#[from] keyspace::TryFromSmartContractError),

    #[error(transparent)]
    InvalidMigration(#[from] migration::TryFromSmartContractError),
}

/// Result of [`View::apply_event`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
