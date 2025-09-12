use {
    super::*,
    crate::{self as cluster, migration, node_operator, EncryptionKey, Node, View},
    derive_more::derive::AsRef,
    futures::{Stream, TryStreamExt as _},
    itertools::Itertools,
    std::{
        array,
        collections::{HashMap, HashSet},
        sync::{Arc, Mutex, MutexGuard},
    },
    tap::Pipe as _,
    time::OffsetDateTime,
    tokio::sync::broadcast,
    tokio_stream::wrappers::BroadcastStream,
};

#[derive(Clone, Default)]
pub struct FakeRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

#[derive(Default)]
struct RegistryInner {
    next_contract_id: u8,
    contracts: HashMap<Address, Arc<Mutex<Inner>>>,
}

impl FakeRegistry {
    pub fn deployer(&self, signer: AccountAddress) -> Deployer {
        Deployer {
            signer,
            registry: self.clone(),
        }
    }

    pub fn connector(&self, signer: AccountAddress) -> Connector {
        Connector {
            signer,
            registry: self.clone(),
        }
    }

    fn inner(&self) -> MutexGuard<'_, RegistryInner> {
        self.inner.lock().unwrap()
    }
}

pub struct Deployer {
    signer: AccountAddress,
    registry: FakeRegistry,
}

impl super::Deployer<FakeSmartContract> for Deployer {
    async fn deploy(
        &self,
        initial_settings: Settings,
        initial_operators: Vec<NodeOperator>,
    ) -> Result<FakeSmartContract, DeploymentError> {
        let operator_indexes: HashSet<_> =
            (0..initial_operators.len()).map(|idx| idx as u8).collect();

        let primary_keyspace = Keyspace {
            operators: operator_indexes,
            replication_strategy: 0,
        };

        let sc_view = ClusterView {
            owner: self.signer,
            settings: initial_settings.clone(),
            node_operators: initial_operators.into_iter().map(Some).collect(),
            keyspaces: [primary_keyspace, Keyspace::default()],
            keyspace_version: 0,
            migration: Migration {
                id: 0,
                pulling_operators: HashSet::default(),
                started_at: 0,
                aborted_at: None,
            },
            maintenance: Maintenance { slot: None },
            cluster_version: 0,
        };

        let view = View::try_from_sc(sc_view.clone(), &Config::dummy())
            .await
            .map_err(|err| DeploymentError(err.to_string()))?;

        let mut registry = self.registry.inner();
        registry.next_contract_id += 1;

        let contract_address = AccountAddress(array::from_fn(|_| registry.next_contract_id));

        let contract = Arc::new(Mutex::new(Inner {
            address: contract_address,
            sc_view,
            view,
            next_migration_id: 1,
            events: broadcast::channel(100).0,
        }));

        let _ = registry
            .contracts
            .insert(contract_address, contract.clone());

        Ok(FakeSmartContract {
            signer: self.signer,
            inner: contract,
        })
    }
}

pub struct Connector {
    signer: AccountAddress,
    registry: FakeRegistry,
}

impl super::Connector<FakeSmartContract> for Connector {
    async fn connect(&self, address: Address) -> Result<FakeSmartContract, ConnectionError> {
        let contract = self
            .registry
            .inner()
            .contracts
            .get(&address)
            .cloned()
            .ok_or(ConnectionError::UnknownContract)?;

        Ok(FakeSmartContract {
            signer: self.signer,
            inner: contract,
        })
    }
}

#[derive(AsRef)]
struct Config {
    #[as_ref]
    encryption_key: EncryptionKey,
}

impl Config {
    fn dummy() -> Self {
        Self {
            encryption_key: crate::testing::encryption_key(),
        }
    }
}

impl crate::Config for Config {
    type SmartContract = FakeSmartContract;
    type KeyspaceShards = ();
    type Node = Node;

    fn new_node(&self, _operator_id: node_operator::Id, node: Node) -> Self::Node {
        node
    }
}

#[derive(Clone)]
pub struct FakeSmartContract {
    signer: AccountAddress,
    inner: Arc<Mutex<Inner>>,
}

impl FakeSmartContract {
    pub fn address(&self) -> Address {
        self.inner.lock().unwrap().address
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }
}

impl FakeSmartContract {
    async fn write<Ev>(&self, f: impl FnOnce(&mut Inner) -> Ev) -> WriteResult<()>
    where
        Ev: Into<Event>,
    {
        let (mut view, event) = {
            let mut this = self.inner();
            (this.view.clone(), f(&mut this).into())
        };

        view = view
            .apply_event(&Config::dummy(), event.clone())
            .await
            .map_err(|err| WriteError::Other(err.to_string()))?;

        let mut this = self.inner();
        this.view = view;

        let _ = this.events.send(event);

        Ok(())
    }
}

struct Inner {
    address: Address,

    sc_view: ClusterView,
    view: cluster::View<Config>,

    next_migration_id: migration::Id,

    events: broadcast::Sender<Event>,
}

impl Inner {
    fn next_migration_id(&mut self) -> migration::Id {
        let id = self.next_migration_id;
        self.next_migration_id += 1;
        id
    }
}

impl super::Write for FakeSmartContract {
    fn signer(&self) -> Option<&AccountAddress> {
        Some(&self.signer)
    }

    async fn start_migration(&self, new_keyspace: Keyspace) -> WriteResult<()> {
        self.write(|this| {
            this.sc_view.migration = Migration {
                id: this.next_migration_id(),
                pulling_operators: new_keyspace.operators.clone(),
                started_at: OffsetDateTime::now_utc().unix_timestamp() as u64,
                aborted_at: None,
            };
            this.sc_view.keyspace_version += 1;
            this.sc_view.keyspaces[this.sc_view.keyspace_version as usize % 2] =
                new_keyspace.clone();
            this.sc_view.cluster_version += 1;

            event::MigrationStarted {
                migration_id: this.sc_view.migration.id,
                new_keyspace,
                at: this.sc_view.migration.started_at,
                cluster_version: this.view.cluster_version + 1,
            }
        })
        .await
    }

    async fn complete_migration(&self, id: migration::Id) -> WriteResult<()> {
        self.write(|this| {
            let operator_idx = this.view.node_operators().get_idx(&self.signer).unwrap();
            this.sc_view
                .migration
                .pulling_operators
                .remove(&operator_idx);

            let event: Event = if this.view.migration.pulling_count() > 1 {
                event::MigrationDataPullCompleted {
                    migration_id: id,
                    operator_id: self.signer,
                    cluster_version: this.view.cluster_version + 1,
                }
                .into()
            } else {
                event::MigrationCompleted {
                    migration_id: id,
                    operator_id: self.signer,
                    cluster_version: this.view.cluster_version + 1,
                }
                .into()
            };

            this.sc_view.cluster_version += 1;

            event
        })
        .await
    }

    async fn abort_migration(&self, id: migration::Id) -> WriteResult<()> {
        self.write(|this| {
            this.sc_view.migration.pulling_operators = HashSet::new();
            this.sc_view.migration.aborted_at =
                Some(OffsetDateTime::now_utc().unix_timestamp() as u64);
            this.sc_view.keyspace_version += 1;
            this.sc_view.cluster_version += 1;

            event::MigrationAborted {
                migration_id: id,
                at: this.sc_view.migration.aborted_at.unwrap(),
                cluster_version: this.view.cluster_version + 1,
            }
        })
        .await
    }

    async fn start_maintenance(&self) -> WriteResult<()> {
        self.write(|this| {
            this.sc_view.maintenance.slot = Some(self.signer);
            this.sc_view.cluster_version += 1;

            event::MaintenanceStarted {
                by: self.signer,
                cluster_version: this.view.cluster_version + 1,
            }
        })
        .await
    }

    async fn finish_maintenance(&self) -> WriteResult<()> {
        self.write(|this| {
            this.sc_view.maintenance.slot = None;
            this.sc_view.cluster_version += 1;

            event::MaintenanceFinished {
                cluster_version: this.view.cluster_version + 1,
            }
        })
        .await
    }

    async fn add_node_operator(&self, operator: NodeOperator) -> WriteResult<()> {
        self.write(|this| {
            let operators = &mut this.sc_view.node_operators;

            let idx = if let Some((idx, _)) = operators.iter().find_position(|slot| slot.is_none())
            {
                operators[idx] = Some(operator.clone());
                idx
            } else {
                operators.push(Some(operator.clone()));
                operators.len() - 1
            };

            this.sc_view.cluster_version += 1;

            event::NodeOperatorAdded {
                idx: idx as u8,
                operator,
                cluster_version: this.view.cluster_version + 1,
            }
        })
        .await
    }

    async fn update_node_operator(&self, operator: NodeOperator) -> WriteResult<()> {
        self.write(|this| {
            let operators = &mut this.sc_view.node_operators;

            let (idx, _) = operators
                .iter()
                .find_position(|slot| slot.is_none())
                .unwrap();

            operators[idx] = Some(operator.clone());
            this.sc_view.cluster_version += 1;

            event::NodeOperatorUpdated {
                operator,
                cluster_version: this.view.cluster_version + 1,
            }
        })
        .await
    }

    async fn remove_node_operator(&self, id: node_operator::Id) -> WriteResult<()> {
        self.write(|this| {
            let operators = &mut this.sc_view.node_operators;

            let (idx, _) = operators
                .iter()
                .find_position(|slot| slot.is_none())
                .unwrap();

            operators[idx] = None;
            this.sc_view.cluster_version += 1;

            event::NodeOperatorRemoved {
                id,
                cluster_version: this.view.cluster_version + 1,
            }
        })
        .await
    }

    async fn update_settings(&self, new_settings: Settings) -> WriteResult<()> {
        self.write(|this| {
            this.sc_view.settings = new_settings.clone();
            this.sc_view.cluster_version += 1;

            event::SettingsUpdated {
                settings: new_settings,
                cluster_version: this.view.cluster_version + 1,
            }
        })
        .await
    }
}

impl super::Read for FakeSmartContract {
    async fn cluster_view(&self) -> ReadResult<ClusterView> {
        Ok(self.inner().sc_view.clone())
    }

    async fn events(
        &self,
    ) -> ReadResult<impl Stream<Item = ReadResult<Event>> + Send + 'static + use<>> {
        BroadcastStream::new(self.inner().events.subscribe())
            .map_err(|err| ReadError::Other(err.to_string()))
            .pipe(Ok)
    }
}

pub fn account_address(n: u8) -> AccountAddress {
    AccountAddress(array::from_fn(|_| n))
}
