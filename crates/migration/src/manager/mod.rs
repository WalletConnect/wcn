use {
    anyhow::{anyhow, Context, Result},
    backoff::ExponentialBackoffBuilder,
    futures::{stream, FutureExt as _, StreamExt, TryFutureExt},
    futures_concurrency::future::Race,
    smallvec::SmallVec,
    std::{
        fmt,
        future::{self, Future},
        pin::pin,
        sync::Arc,
        time::Duration,
    },
    tap::{Pipe as _, TapFallible},
    time::OffsetDateTime,
    wcn_cluster::{
        self as cluster,
        keyspace::{self, ReplicaSet},
        migration,
        node_operator,
        smart_contract::{self, Write},
        Cluster,
        NodeOperator,
        PeerId,
        SmartContract,
    },
    wcn_storage_api::{self as storage_api, StorageApi},
};

#[cfg(test)]
mod test;

/// Migration [`Manager`] config.
pub trait Config:
    cluster::Config<
        SmartContract: SmartContract,
        KeyspaceShards = keyspace::Shards,
        Node: AsRef<PeerId>,
    > + Clone
{
    /// Type of the outbound connections to WCN Replicas.
    type OutboundReplicaConnection: StorageApi;

    /// Type of the outbound connection to the WCN Database.
    type OutboundDatabaseConnection: StorageApi + Clone;

    /// Extracts [`Config::OutboundReplicaConnection`] from the provided
    /// node.
    fn get_replica_connection<'a>(
        &self,
        node: &'a Self::Node,
    ) -> &'a Self::OutboundReplicaConnection;
}

/// WCN Migration Manager.
///
/// Manages data migration activities of a specific node operator within a WCN
/// Cluster.
#[derive(Clone)]
pub struct Manager<C: Config> {
    node_operator_id: node_operator::Id,

    config: Arc<C>,
    cluster: Cluster<C>,
    database: C::OutboundDatabaseConnection,
}

impl<C: Config> Manager<C> {
    /// Tries to creates a new migration [`Manager`].
    ///
    /// Returns `None` if [`smart_contract`] doesn't have a configured signer.
    pub fn new(
        config: C,
        cluster: Cluster<C>,
        database: C::OutboundDatabaseConnection,
    ) -> Option<Self> {
        let node_operator_id = *cluster.smart_contract().signer()?;

        Some(Self {
            node_operator_id,
            config: Arc::new(config),
            cluster,
            database,
        })
    }

    /// Runs a task managing all data migration related activities of a node
    /// operator.
    ///
    /// There should only be a single such task running at any point in time
    /// across all node operator nodes / infrastructure services.
    pub fn run(
        &self,
        shutdown_fut: impl Future<Output = ()> + Send,
    ) -> impl Future<Output = ()> + Send {
        Task {
            manager: self.clone(),
        }
        .run(shutdown_fut)
    }
}

struct Task<C: Config> {
    manager: Manager<C>,
}

impl<C> Task<C>
where
    C: Config<SmartContract: smart_contract::Write, KeyspaceShards = keyspace::Shards>,
{
    async fn run(self, shutdown_fut: impl Future) {
        let mut shutdown_fut = pin!(shutdown_fut.map(|_| Event::Shutdown).fuse());
        let mut is_shutting_down = false;

        let mut watch = self.cluster().watch();

        let mut state = State::Idle;
        let mut state_fut = pin!(self.state_future(state));

        loop {
            let cluster_update_fut = watch.cluster_updated().map(|_| Event::ClusterUpdate);

            let new_state = match (cluster_update_fut, &mut state_fut, &mut shutdown_fut)
                .race()
                .await
            {
                Event::ClusterUpdate => self.sync_state(state),
                Event::StateTransition(state) => Some(state),

                // OK to shutdown right now.
                Event::Shutdown if state.can_shutdown() => return,

                // We are not ready to shutdown yet.
                Event::Shutdown => {
                    is_shutting_down = true;
                    continue;
                }
            };

            if let Some(new_state) = new_state {
                tracing::info!("{state:?} -> {new_state:?}");

                if is_shutting_down && state.can_shutdown() {
                    return;
                }

                state = new_state;
                state_fut.set(self.state_future(state));
            }
        }
    }

    fn sync_state(&self, current: State) -> Option<State> {
        let local_migration_id = current.migration_id();
        let cluster_migration_id = self.cluster().view().migration().map(|mig| mig.id());

        match (local_migration_id, cluster_migration_id) {
            // State is up to date.
            (Some(a), Some(b)) if a == b => None,
            (None, None) => None,

            // We are doing something, but cluster no longer has an ongoing migration.
            // Drop everything and go back to `Idle`.
            (Some(_), None) => Some(State::Idle),

            // We are either `Idle` or handling a wrong migration.
            // Kick-off a new migration process.
            (_, Some(id)) => Some(State::TransferringData(id)),
        }
    }

    async fn state_future(&self, state: State) -> Event {
        retry(|| async {
            match state {
                State::TransferringData(migration_id) => self
                    .transfer_data(migration_id)
                    .await
                    .context("Manager::transfer_data"),
                State::CompletingMigration(migration_id) => self
                    .complete_migration(migration_id)
                    .await
                    .context("Manager::complete_migration"),
                State::AwaitingMigrationCompletion(migration_id) => self
                    .await_migration_completion(migration_id)
                    .await
                    .context("Manager::await_migration_completion"),
                State::Idle => future::pending().await,
            }
            .tap_err(|err| tracing::error!(?err))
        })
        .map(Event::StateTransition)
        .await
    }

    async fn transfer_data(&self, migration_id: migration::Id) -> Result<State> {
        let start_at = self
            .cluster()
            .using_view(wcn_cluster::View::data_pull_scheduled_after)
            .context("Missing migration")?;

        let now = OffsetDateTime::now_utc();

        if now < start_at {
            let duration = start_at - now;
            let duration_ms = duration.whole_milliseconds();

            tracing::info!("Data transfer scheduled at {}ms from now", duration_ms);
            tokio::time::sleep(duration.unsigned_abs()).await;
        }

        let cluster_view = self.cluster().view();
        let keyspace_version = cluster_view.keyspace_version();

        let primary_shards = cluster_view.primary_keyspace_shards();
        let secondary_shards = cluster_view
            .secondary_keyspace_shards()
            .ok_or_else(|| anyhow!("Missing secondary keyspace shards"))?;

        // a separate closure because otherwise rustfmt breaks and refuses to format
        // this function
        let log_err = |err, shard_id, source_id| tracing::warn!(?err, %shard_id, %source_id, "Failed to transfer shard");

        primary_shards
            .zip(secondary_shards)
            .filter_map(|((shard_id, primary), (_, secondary))| {
                let old = primary.replica_set();
                let new = secondary.replica_set();

                let added_operators = replica_set_difference(new, old);

                let idx = added_operators
                    .iter()
                    .position(|op| op.id == self.manager.node_operator_id)?;

                // Skip the shard if this node operator is not being newly added to it.
                let removed_operators = replica_set_difference(old, new);

                // Pull data only from the operator being replaced to ensure consistency.
                // TODO: We should have a way to override this behaviour for special
                // circumstances, for example if a node operator is completely dead and we are
                // forcefully removing it.
                let source = removed_operators[idx];

                Some((shard_id, source))
            })
            .pipe(stream::iter)
            .for_each_concurrent(
                Some(cluster_view.settings().migration_concurrency.into()),
                |(shard_id, source)| {
                    retry(move || {
                        self.transfer_shard(shard_id, source, keyspace_version)
                            .map_err(move |err| log_err(err, shard_id, source.id))
                    })
                },
            )
            .await;

        Ok(State::CompletingMigration(migration_id))
    }

    async fn transfer_shard(
        &self,
        shard_id: keyspace::ShardId,
        source_operator: &NodeOperator<C::Node>,
        keyspace_version: u64,
    ) -> storage_api::Result<()> {
        use storage_api::ErrorKind;

        let keyrange = keyspace::keyrange(shard_id);

        let mut res = Err(storage_api::Error::internal());

        // Retry transport errors using different nodes.
        for node in source_operator.nodes_lb_iter() {
            let conn = self.manager.config.get_replica_connection(node);
            res = conn.read_data(keyrange.clone(), keyspace_version).await;

            if matches!(&res, Err(err) if err.kind() == ErrorKind::Transport) {
                continue;
            };

            return self.database().write_data(res?).await;
        }

        res.map(drop)
    }

    async fn complete_migration(&self, migration_id: migration::Id) -> Result<State> {
        use cluster::CompleteMigrationError as Error;

        match self.cluster().complete_migration(migration_id).await {
            Ok(_) => {}
            // This error is theoretically possible under an extreme race condition when something
            // else completes the migration.
            Err(Error::OperatorNotPulling(_)) => {
                return Ok(State::AwaitingMigrationCompletion(migration_id))
            }
            Err(
                err @ (Error::UnknownOperator(_)
                | Error::NoMigration(_)
                | Error::WrongMigrationId(_)
                | Error::NoSigner(_)
                | Error::SmartContract(_)),
            ) => return Err(err.into()),
        };

        // Wait until our commit to SC is observable.
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if !self
                    .cluster()
                    .using_view(|view| view.is_pulling(&self.manager.node_operator_id))
                {
                    return State::AwaitingMigrationCompletion(migration_id);
                }

                tokio::time::sleep(Duration::from_secs(10)).await
            }
        })
        .await
        .context("Waiting for is_pulling == false")
    }

    async fn await_migration_completion(&self, migration_id: migration::Id) -> Result<State> {
        loop {
            // We have completed the migration already, but somehow the cluster shows again
            // that we are pulling.
            // It indicates that our commit to the SC didn't go through somehow, probably
            // because of a chain reorg.
            //
            // Go back to `CompletingMigration` state.
            if self
                .cluster()
                .using_view(|view| view.is_pulling(&self.manager.node_operator_id))
            {
                tracing::warn!("Going back to `State::CompletingMigration`");
                return Ok(State::CompletingMigration(migration_id));
            }

            // Just loop enlessly.
            //
            // The task will be canceled once we receive `ClusterUpdate` and there's no
            // longer an ongoing migration within the cluster.
            tokio::time::sleep(Duration::from_secs(10)).await
        }
    }

    fn cluster(&self) -> &Cluster<C> {
        &self.manager.cluster
    }

    fn database(&self) -> &C::OutboundDatabaseConnection {
        &self.manager.database
    }
}

enum Event {
    ClusterUpdate,
    StateTransition(State),
    Shutdown,
}

#[derive(Clone, Copy, Debug)]
enum State {
    TransferringData(migration::Id),
    CompletingMigration(migration::Id),
    AwaitingMigrationCompletion(migration::Id),
    Idle,
}

impl State {
    fn migration_id(self) -> Option<migration::Id> {
        Some(match self {
            State::TransferringData(migration_id) => migration_id,
            State::CompletingMigration(migration_id) => migration_id,
            State::AwaitingMigrationCompletion(migration_id) => migration_id,
            State::Idle => return None,
        })
    }

    fn can_shutdown(&self) -> bool {
        match self {
            State::TransferringData(_) | State::CompletingMigration(_) => false,
            State::AwaitingMigrationCompletion(_) | State::Idle => true,
        }
    }
}

async fn retry<Ok, E, F, Fut>(f: F) -> Ok
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<Ok, E>>,
    E: fmt::Debug,
{
    let backoff = ExponentialBackoffBuilder::new()
        .with_randomization_factor(0.5)
        .with_initial_interval(Duration::from_secs(1))
        .with_max_interval(Duration::from_secs(60))
        .with_max_elapsed_time(None)
        .build();

    let f = || async { f().await.map_err(backoff::Error::transient) };

    // NOTE(unwrap): we use `.with_max_elapsed_time(None)`, the error won't be
    // emitted.
    backoff::future::retry(backoff, f).await.unwrap()
}

// Returns `NodeOperator`s which are in `a`, but not in `b`.
fn replica_set_difference<'a, T>(
    a: &ReplicaSet<&'a NodeOperator<T>>,
    b: &ReplicaSet<&NodeOperator<T>>,
) -> SmallVec<[&'a NodeOperator<T>; 5]> {
    let mut diff = SmallVec::new();

    for operator in a {
        if !b.iter().any(|op| op.id == operator.id) {
            diff.push(*operator);
        }
    }

    diff
}
