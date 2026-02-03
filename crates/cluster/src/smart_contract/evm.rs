//! EVM implementation of [`SmartContract`](super::SmartContract).

use {
    super::*,
    crate::{migration, node_operator, smart_contract},
    alloy::{
        contract::{CallBuilder, CallDecoder},
        network::EthereumWallet,
        primitives::{Address as AlloyAddress, U256},
        providers::{DynProvider, Provider, ProviderBuilder, WsConnect},
        rpc::types::{Filter, Log},
        signers::local::PrivateKeySigner,
        sol_types::{SolCall, SolEventInterface, SolInterface},
        transports::http::reqwest,
    },
    derive_where::derive_where,
    futures::{Stream, StreamExt},
    std::{collections::HashSet, fmt, time::Duration},
};

mod bindings {
    alloy::sol!(
        #[sol(rpc)]
        #[derive(Debug)]
        Cluster,
        "../../contracts/out/Cluster.sol/Cluster.json",
    );

    alloy::sol!(
        #[sol(rpc)]
        #[derive(Debug)]
        ERC1967Proxy,
        "../../contracts/out/ERC1967Proxy.sol/ERC1967Proxy.json",
    );
}

type AlloyContract = bindings::Cluster::ClusterInstance<DynProvider, alloy::network::Ethereum>;

/// RPC provider for EVM chains.
pub struct RpcProvider {
    signer: Option<Signer>,
    alloy: DynProvider,
}

impl RpcProvider {
    /// Creates a new [`RpcProvider`].
    pub async fn new(url: RpcUrl, signer: Signer) -> Result<Self, RpcProviderCreationError> {
        let wallet: EthereumWallet = match &signer.kind {
            SignerKind::PrivateKey(key) => key.clone().into(),
        };

        let builder = ProviderBuilder::new().wallet(wallet);

        let provider = match url.0.scheme() {
            "ws" | "wss" => builder.connect_ws(Self::alloy_ws_connect(url)).await?,
            "http" | "https" => builder.connect_http(url.0),
            _ => {
                return Err(RpcProviderCreationError(format!(
                    "Unsupported URL scheme: {}",
                    url.0.scheme()
                )));
            }
        };

        Ok(RpcProvider {
            signer: Some(signer),
            alloy: DynProvider::new(provider),
        })
    }

    /// Creates a new read-only [`RpcProvider`].
    pub async fn new_ro(url: RpcUrl) -> Result<RpcProvider, RpcProviderCreationError> {
        let provider = ProviderBuilder::new()
            .connect_ws(Self::alloy_ws_connect(url))
            .await?;

        Ok(RpcProvider {
            signer: None,
            alloy: DynProvider::new(provider),
        })
    }

    fn alloy_ws_connect(url: RpcUrl) -> WsConnect {
        // make sure that it "never" stops retrying
        WsConnect::new(url.0).with_max_retries(u32::MAX)
    }
}

/// EVM implementation of WCN Cluster smart contract.
#[derive(Clone)]
pub struct SmartContract {
    signer: Option<Signer>,
    alloy: AlloyContract,
}

impl SmartContract {
    pub fn address(&self) -> Address {
        (*self.alloy.address()).into()
    }
}

impl Deployer<SmartContract> for RpcProvider {
    async fn deploy(
        &self,
        initial_settings: Settings,
        initial_operators: Vec<NodeOperator>,
    ) -> Result<SmartContract, DeploymentError> {
        let settings: bindings::Cluster::Settings = initial_settings.into();
        let operators: Vec<bindings::Cluster::NodeOperator> =
            initial_operators.into_iter().map(Into::into).collect();

        let contract = bindings::Cluster::deploy(self.alloy.clone()).await?;

        let init_call = bindings::Cluster::initializeCall::new((settings, operators)).abi_encode();

        let proxy = bindings::ERC1967Proxy::deploy(
            self.alloy.clone(),
            *contract.address(),
            init_call.into(),
        )
        .await?;

        // Try to make sure that the code is actually there before we return from this
        // function. We are doing contract calls right after and they fail with
        // `ZeroData` errors otherwise.
        for _ in 0..10 {
            let code = proxy
                .provider()
                .get_code_at(*proxy.address())
                .await
                .map_err(|err| DeploymentError(format!("Provider::get_code_at: {err:?}")))?;

            if !code.is_empty() {
                break;
            }

            tokio::time::sleep(Duration::from_millis(500)).await
        }

        Ok(SmartContract {
            signer: self.signer.clone(),
            alloy: bindings::Cluster::new(*proxy.address(), self.alloy.clone()),
        })
    }
}

impl Connector<SmartContract> for RpcProvider {
    async fn connect(&self, address: Address) -> Result<SmartContract, ConnectionError> {
        let address = address.into();

        let signer = self.signer.clone();

        let code = self
            .alloy
            .get_code_at(address)
            .await
            .map_err(|err| ConnectionError::Other(err.to_string()))?;

        if code.is_empty() {
            return Err(ConnectionError::UnknownContract);
        }

        // TODO: figure out how to check this with Proxy contract
        // if code != bindings::Cluster::BYTECODE {
        //     return Err(ConnectionError::WrongContract);
        // }

        Ok(SmartContract {
            signer,
            alloy: bindings::Cluster::new(address, self.alloy.clone()),
        })
    }
}

impl smart_contract::Write for SmartContract {
    fn signer(&self) -> Option<&AccountAddress> {
        self.signer.as_ref().map(Signer::address)
    }

    async fn start_migration(&self, new_keyspace: Keyspace) -> WriteResult<()> {
        let new_keyspace: bindings::Cluster::Keyspace = new_keyspace.into();
        check_receipt(self.alloy.startMigration(new_keyspace)).await
    }

    async fn complete_migration(&self, id: migration::Id) -> WriteResult<()> {
        check_receipt(self.alloy.completeMigration(id)).await
    }

    async fn abort_migration(&self, id: migration::Id) -> WriteResult<()> {
        check_receipt(self.alloy.abortMigration(id)).await
    }

    async fn start_maintenance(&self) -> WriteResult<()> {
        check_receipt(self.alloy.setMaintenance(true)).await
    }

    async fn finish_maintenance(&self) -> WriteResult<()> {
        check_receipt(self.alloy.setMaintenance(false)).await
    }

    async fn add_node_operator(&self, operator: NodeOperator) -> WriteResult<()> {
        check_receipt(self.alloy.addNodeOperator(operator.into())).await
    }

    async fn update_node_operator(&self, operator: NodeOperator) -> WriteResult<()> {
        let operator: bindings::Cluster::NodeOperator = operator.into();
        check_receipt(
            self.alloy
                .updateNodeOperatorData(operator.addr, operator.data),
        )
        .await
    }

    async fn remove_node_operator(&self, id: node_operator::Id) -> WriteResult<()> {
        check_receipt(self.alloy.removeNodeOperator(id.into())).await
    }

    async fn update_settings(&self, new_settings: Settings) -> WriteResult<()> {
        check_receipt(self.alloy.updateSettings(new_settings.into())).await
    }

    async fn transfer_ownership(&self, new_owner: AccountAddress) -> WriteResult<()> {
        check_receipt(self.alloy.transferOwnership(new_owner.into())).await
    }

    async fn accept_ownership(&self) -> WriteResult<()> {
        check_receipt(self.alloy.acceptOwnership()).await
    }
}

async fn check_receipt<D>(call: CallBuilder<&DynProvider, D>) -> WriteResult<()>
where
    D: CallDecoder,
{
    let receipt = call
        .send()
        .await?
        .with_timeout(Some(Duration::from_secs(30)))
        .get_receipt()
        .await?;

    if !receipt.status() {
        return Err(WriteError::Revert(format!("{}", receipt.transaction_hash)));
    }

    Ok(())
}

impl smart_contract::Read for SmartContract {
    async fn cluster_view(&self) -> ReadResult<ClusterView> {
        self.alloy.getView().call().await?.try_into()
    }

    async fn events(
        &self,
    ) -> ReadResult<impl Stream<Item = ReadResult<Event>> + Send + 'static + use<>> {
        let filter = Filter::new().address(*self.alloy.address());
        let provider = self.alloy.provider().clone();

        Ok(provider
            .subscribe_logs(&filter)
            .await?
            .into_stream()
            .filter_map(move |log| {
                let provider = provider.clone();
                async move { Event::try_from(log, &provider).await.transpose() }
            }))
    }
}

async fn get_block_timestamp(log: &Log, provider: &DynProvider) -> ReadResult<u64> {
    if let Some(ts) = log.block_timestamp {
        return Ok(ts);
    }

    let Some(number) = log.block_number else {
        return Err(ReadError::Other("Missing block number".into()));
    };

    let block = provider
        .get_block_by_number(number.into())
        .await?
        .ok_or_else(|| ReadError::Other(format!("Block(number: {number}) not found")))?;

    Ok(block.into_header().timestamp)
}

impl Event {
    async fn try_from(log: Log, provider: &DynProvider) -> ReadResult<Option<Event>> {
        use bindings::Cluster::ClusterEvents as Event;

        let evt =
            Event::decode_log(&log.inner).map_err(|err| ReadError::InvalidData(err.to_string()))?;

        Ok(Some(match evt.data {
            Event::MaintenanceToggled(evt) => evt.into(),
            Event::MigrationAborted(evt) => {
                self::Event::from_migration_aborted(evt, get_block_timestamp(&log, provider).await?)
            }
            Event::MigrationCompleted(evt) => evt.into(),
            Event::MigrationDataPullCompleted(evt) => evt.into(),
            Event::MigrationStarted(evt) => {
                self::Event::from_migration_started(evt, get_block_timestamp(&log, provider).await?)
            }
            Event::NodeOperatorAdded(evt) => evt.into(),
            Event::NodeOperatorRemoved(evt) => evt.into(),
            Event::NodeOperatorUpdated(evt) => evt.into(),
            Event::SettingsUpdated(evt) => evt.into(),
            Event::ClusterInitialized(_)
            | Event::Initialized(_)
            | Event::OwnershipTransferStarted(_)
            | Event::OwnershipTransferred(_)
            | Event::Upgraded(_) => return Ok(None),
        }))
    }
}

impl Event {
    fn from_migration_started(
        evt: bindings::Cluster::MigrationStarted,
        block_timestamp: u64,
    ) -> Self {
        Self::MigrationStarted(event::MigrationStarted {
            migration_id: evt.id,
            new_keyspace: evt.newKeyspace.into(),
            at: block_timestamp,
            cluster_version: evt.version,
        })
    }

    fn from_migration_aborted(
        evt: bindings::Cluster::MigrationAborted,
        block_timestamp: u64,
    ) -> Self {
        Self::MigrationAborted(event::MigrationAborted {
            migration_id: evt.id,
            at: block_timestamp,
            cluster_version: evt.version,
        })
    }
}

impl From<bindings::Cluster::MigrationDataPullCompleted> for Event {
    fn from(evt: bindings::Cluster::MigrationDataPullCompleted) -> Self {
        Self::MigrationDataPullCompleted(event::MigrationDataPullCompleted {
            migration_id: evt.id,
            operator_id: evt.operator.into(),
            cluster_version: evt.version,
        })
    }
}

impl From<bindings::Cluster::MigrationCompleted> for Event {
    fn from(evt: bindings::Cluster::MigrationCompleted) -> Self {
        Self::MigrationCompleted(event::MigrationCompleted {
            migration_id: evt.id,
            operator_id: evt.operator.into(),
            cluster_version: evt.version,
        })
    }
}

impl From<bindings::Cluster::MaintenanceToggled> for Event {
    fn from(evt: bindings::Cluster::MaintenanceToggled) -> Self {
        if evt.active {
            Self::MaintenanceStarted(event::MaintenanceStarted {
                by: evt.operator.into(),
                cluster_version: evt.version,
            })
        } else {
            Self::MaintenanceFinished(event::MaintenanceFinished {
                cluster_version: evt.version,
            })
        }
    }
}

impl From<bindings::Cluster::NodeOperatorAdded> for Event {
    fn from(evt: bindings::Cluster::NodeOperatorAdded) -> Self {
        let operator = bindings::Cluster::NodeOperator {
            addr: evt.operator,
            data: evt.operatorData,
        };

        Self::NodeOperatorAdded(event::NodeOperatorAdded {
            idx: evt.slot,
            operator: operator.into(),
            cluster_version: evt.version,
        })
    }
}

impl From<bindings::Cluster::NodeOperatorUpdated> for Event {
    fn from(evt: bindings::Cluster::NodeOperatorUpdated) -> Self {
        let operator = bindings::Cluster::NodeOperator {
            addr: evt.operator,
            data: evt.operatorData,
        };

        Self::NodeOperatorUpdated(event::NodeOperatorUpdated {
            operator: operator.into(),
            cluster_version: evt.version,
        })
    }
}

impl From<bindings::Cluster::NodeOperatorRemoved> for Event {
    fn from(evt: bindings::Cluster::NodeOperatorRemoved) -> Self {
        Self::NodeOperatorRemoved(event::NodeOperatorRemoved {
            id: evt.operator.into(),
            cluster_version: evt.version,
        })
    }
}

impl From<bindings::Cluster::SettingsUpdated> for Event {
    fn from(evt: bindings::Cluster::SettingsUpdated) -> Self {
        Self::SettingsUpdated(event::SettingsUpdated {
            settings: evt.newSettings.into(),
            cluster_version: evt.version,
        })
    }
}

impl From<NodeOperator> for bindings::Cluster::NodeOperator {
    fn from(op: NodeOperator) -> Self {
        Self {
            addr: op.id.into(),
            data: op.data.into(),
        }
    }
}

impl From<bindings::Cluster::NodeOperator> for NodeOperator {
    fn from(op: bindings::Cluster::NodeOperator) -> Self {
        NodeOperator {
            id: op.addr.into(),
            data: op.data.into(),
        }
    }
}

impl From<Settings> for bindings::Cluster::Settings {
    fn from(settings: Settings) -> Self {
        Self {
            maxOperatorDataBytes: settings.max_node_operator_data_bytes,
            minOperators: const { crate::MIN_OPERATORS },
            extra: settings.extra.into(),
        }
    }
}

impl From<bindings::Cluster::Settings> for Settings {
    fn from(settings: bindings::Cluster::Settings) -> Self {
        Self {
            max_node_operator_data_bytes: settings.maxOperatorDataBytes,
            extra: settings.extra.into(),
        }
    }
}

impl From<Keyspace> for bindings::Cluster::Keyspace {
    fn from(keyspace: Keyspace) -> Self {
        let mut operator_bitmask = U256::default();
        for idx in keyspace.operators {
            operator_bitmask.set_bit(idx as usize, true);
        }

        Self {
            operatorBitmask: operator_bitmask,
            replicationStrategy: keyspace.replication_strategy,
        }
    }
}

impl From<bindings::Cluster::Keyspace> for Keyspace {
    fn from(keyspace: bindings::Cluster::Keyspace) -> Self {
        Self {
            operators: bitmask_to_hashset(keyspace.operatorBitmask),
            replication_strategy: keyspace.replicationStrategy,
        }
    }
}

impl From<bindings::Cluster::Migration> for Migration {
    fn from(migration: bindings::Cluster::Migration) -> Self {
        Self {
            id: migration.id,
            pulling_operators: bitmask_to_hashset(migration.pullingOperatorBitmask),
            started_at: migration.startedAt,
            aborted_at: if migration.abortedAt >= migration.startedAt
                && migration.pullingOperatorBitmask == 0
            {
                Some(migration.abortedAt)
            } else {
                None
            },
        }
    }
}

impl TryFrom<bindings::Cluster::ClusterView> for ClusterView {
    type Error = ReadError;

    fn try_from(view: bindings::Cluster::ClusterView) -> Result<Self, Self::Error> {
        let node_operators = view
            .operatorSlots
            .into_iter()
            .map(|slot| {
                if slot.addr.is_zero() {
                    return None;
                }

                let operator = bindings::Cluster::NodeOperator {
                    addr: slot.addr,
                    data: slot.data,
                };

                Some(operator.into())
            })
            .collect();

        let maintenance = Maintenance {
            slot: if view.maintenanceSlot.is_zero() {
                None
            } else {
                Some(view.maintenanceSlot.into())
            },
        };

        Ok(Self {
            owner: view.owner.into(),
            settings: view.settings.into(),
            node_operators,
            keyspaces: view.keyspaces.map(Into::into),
            keyspace_version: view.keyspaceVersion,
            migration: view.migration.into(),
            maintenance,
            cluster_version: view.version,
        })
    }
}

impl From<alloy::contract::Error> for DeploymentError {
    fn from(err: alloy::contract::Error) -> Self {
        Self(format!("{err:?}"))
    }
}

impl<E: fmt::Debug> From<alloy::transports::RpcError<E>> for ReadError {
    fn from(err: alloy::transports::RpcError<E>) -> Self {
        ReadError::Transport(format!("{err:?}"))
    }
}

impl From<alloy::providers::PendingTransactionError> for WriteError {
    fn from(err: alloy::providers::PendingTransactionError) -> Self {
        if let alloy::providers::PendingTransactionError::TransportError(err) = &err {
            if let Some(err) = try_decode_error(err) {
                return WriteError::Revert(format!("{err:?}"));
            }
        }

        Self::Other(format!("{err:?}"))
    }
}

impl From<alloy::contract::Error> for WriteError {
    fn from(err: alloy::contract::Error) -> Self {
        match err {
            alloy::contract::Error::TransportError(err) => {
                if let Some(err) = try_decode_error(&err) {
                    Self::Revert(format!("{err:?}"))
                } else {
                    Self::Transport(format!("{err:?}"))
                }
            }
            _ => Self::Other(format!("{err:?}")),
        }
    }
}

fn try_decode_error(
    err: &alloy::transports::TransportError,
) -> Option<bindings::Cluster::ClusterErrors> {
    let data = match err {
        alloy::transports::RpcError::ErrorResp(resp) => resp.data.as_ref()?,
        _ => return None,
    };

    let data: String = serde_json::from_str(data.get()).ok()?;
    let data = data.strip_prefix("0x")?;
    let bytes = const_hex::decode(data).ok()?;

    bindings::Cluster::ClusterErrors::abi_decode_validate(&bytes).ok()
}

impl From<alloy::contract::Error> for ReadError {
    fn from(err: alloy::contract::Error) -> Self {
        match err {
            alloy::contract::Error::TransportError(err) => Self::Transport(format!("{err:?}")),
            _ => Self::Other(format!("{err:?}")),
        }
    }
}

impl From<alloy::sol_types::Error> for ReadError {
    fn from(err: alloy::sol_types::Error) -> Self {
        ReadError::InvalidData(format!("{err:?}"))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{_0}")]
pub struct RpcProviderCreationError(String);

impl From<alloy::transports::TransportError> for RpcProviderCreationError {
    fn from(err: alloy::transports::TransportError) -> Self {
        Self(format!("alloy transport: {err:?}"))
    }
}

fn bitmask_to_hashset(bitmask: U256) -> HashSet<u8> {
    let mut set = HashSet::new();
    for idx in 0..=u8::MAX {
        if bitmask.bit(idx as usize) {
            set.insert(idx);
        }
    }
    set
}

impl From<AccountAddress> for AlloyAddress {
    fn from(addr: AccountAddress) -> Self {
        AlloyAddress::from_slice(&addr.0)
    }
}

impl From<AlloyAddress> for AccountAddress {
    fn from(addr: AlloyAddress) -> Self {
        AccountAddress(addr.into())
    }
}

/// RPC provider URL.
#[derive(Clone, Debug)]
pub struct RpcUrl(reqwest::Url);

#[derive(Debug, thiserror::Error)]
#[error("Invalid RPC URL: {0:?}")]
pub struct InvalidRpcUrlError(String);

impl FromStr for RpcUrl {
    type Err = InvalidRpcUrlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url = reqwest::Url::from_str(s)
            .map(Self)
            .map_err(|err| InvalidRpcUrlError(err.to_string()))?;

        if !["ws", "wss"].contains(&url.0.scheme()) {
            return Err(InvalidRpcUrlError(
                "Only ws:// and wss:// are supported".to_string(),
            ));
        }

        Ok(url)
    }
}

/// Transaction signer.
#[derive(Clone)]
#[derive_where(Debug)]
pub struct Signer {
    address: AccountAddress,

    #[derive_where(skip)]
    kind: SignerKind,
}

impl FromStr for Signer {
    type Err = InvalidPrivateKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from_private_key(s)
    }
}

impl Signer {
    pub fn try_from_private_key(hex: &str) -> Result<Self, InvalidPrivateKeyError> {
        let private_key = PrivateKeySigner::from_str(hex)
            .map_err(|err| InvalidPrivateKeyError(format!("{err:?}")))?;

        Ok(Self {
            address: private_key.address().into(),
            kind: SignerKind::PrivateKey(private_key),
        })
    }

    /// Returns [`AccountAddress`] of this [`Signer`].
    pub fn address(&self) -> &AccountAddress {
        &self.address
    }
}

#[derive(Clone)]
enum SignerKind {
    PrivateKey(PrivateKeySigner),
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid private key: {0:?}")]
pub struct InvalidPrivateKeyError(String);
