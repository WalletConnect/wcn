use {
    client::BaseClient,
    derive_where::derive_where,
    futures::FutureExt as _,
    futures_concurrency::future::Race as _,
    std::{collections::HashSet, net::SocketAddrV4, sync::Arc, time::Duration},
    tap::Pipe,
    wc::metrics::{self, enum_ordinalize::Ordinalize},
    wcn_cluster::smart_contract,
    wcn_rpc::client::{Api, Connection},
    wcn_storage_api::{
        MapEntryBorrowed,
        Record,
        RecordBorrowed,
        RecordExpiration,
        RecordVersion,
        operation as op,
    },
};
pub use {
    encryption::{Error as EncryptionError, Key as EncryptionKey},
    libp2p_identity::{Keypair, PeerId},
    op::Output as OperationOutput,
    smart_contract::ReadError as SmartContractError,
    wcn_cluster::{
        CreationError as ClusterCreationError,
        EncryptionKey as ClusterKey,
        Node,
        node_operator::Id as NodeOperatorId,
    },
    wcn_cluster_api::Error as ClusterError,
    wcn_rpc::client::Error as RpcError,
    wcn_storage_api::{
        Error as CoordinatorError,
        ErrorKind as CoordinatorErrorKind,
        MapPage,
        Namespace,
    },
};

mod client;
mod cluster;
mod encryption;

#[derive(Debug, thiserror::Error, strum::IntoStaticStr)]
pub enum Error {
    #[error("No available nodes")]
    NoAvailableNodes,

    #[error("Retries exhausted")]
    RetriesExhausted,

    #[error("Failed to initialize cluster: {0}")]
    ClusterCreation(#[from] ClusterCreationError),

    #[error("RPC client error: {0}")]
    Rpc(#[from] RpcError),

    #[error("Cluster API error: {0}")]
    ClusterApi(#[from] ClusterError),

    #[error("Coordinator API error: {0}")]
    CoordinatorApi(#[from] CoordinatorError),

    #[error("Failed to read smart contract: {0}")]
    SmartContract(#[from] SmartContractError),

    #[error("Encryption failed: {0}")]
    Encryption(#[from] EncryptionError),

    #[error("Invalid response type")]
    InvalidResponseType,

    #[error("Internal error: {0}")]
    Internal(String),
}

impl Error {
    pub fn internal(err: impl ToString) -> Self {
        Self::Internal(err.to_string())
    }

    pub fn code(&self) -> &'static str {
        self.into()
    }
}

pub struct Config {
    /// [`Keypair`] to be used for RPC clients.
    pub keypair: Keypair,

    /// Symmetrical encryption key used for accessing the on-chain data.
    pub cluster_key: ClusterKey,

    /// Timeout of establishing a network connection.
    pub connection_timeout: Duration,

    /// Timeout of a storage API operation.
    pub operation_timeout: Duration,

    /// Reconnection interval of the RPC client.
    pub reconnect_interval: Duration,

    /// Concurrency limit for the storage operations.
    pub max_concurrent_rpcs: u32,

    /// For how long an outbound connection is allowed to be idle before it
    /// gets timed out.
    pub max_idle_connection_timeout: Duration,

    /// Maximum number of retries in case of a failed operation.
    pub max_retries: usize,

    /// [`PeerAddr`]s of the bootstrap nodes.
    pub nodes: Vec<PeerAddr>,

    /// List of trusted node operators to use with the cluster API. If the list
    /// is empty, all nodes in the cluster may be used.
    pub trusted_operators: HashSet<NodeOperatorId>,
}

#[derive(Debug, Clone)]
pub struct PeerAddr {
    pub id: PeerId,
    pub addr: SocketAddrV4,
}

pub struct Builder<T> {
    config: Config,
    observer: T,
    encryption_key: Option<EncryptionKey>,
}

impl Builder<()> {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            observer: (),
            encryption_key: None,
        }
    }
}

impl<T> Builder<T> {
    pub fn with_observer<O>(self, observer: O) -> Builder<O>
    where
        O: RequestObserver,
    {
        Builder {
            config: self.config,
            observer,
            encryption_key: self.encryption_key,
        }
    }

    pub fn with_encryption(mut self, key: EncryptionKey) -> Self {
        self.encryption_key = Some(key);
        self
    }
}

impl<T> Builder<T>
where
    T: RequestObserver,
{
    pub async fn build(self) -> Result<Client<T>, Error> {
        Ok(Client {
            inner: BaseClient::new(self.config, self.observer, self.encryption_key)
                .await?
                .pipe(Arc::new),
        })
    }
}

#[derive_where(Clone)]
pub struct Client<T: RequestObserver = ()> {
    inner: Arc<BaseClient<T>>,
}

impl Client<()> {
    pub fn builder(config: Config) -> Builder<()> {
        Builder::new(config)
    }
}

impl<T> Client<T>
where
    T: RequestObserver + Send + Sync,
{
    pub async fn get(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<Record>, Error> {
        self.execute_request::<Option<Record>>(op::GetBorrowed {
            namespace,
            key: key.as_ref(),
            keyspace_version: None,
        })
        .await
    }

    pub async fn set(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
        ttl: impl Into<RecordExpiration>,
    ) -> Result<(), Error> {
        self.execute_request(op::SetBorrowed {
            namespace,
            key: key.as_ref(),
            record: RecordBorrowed {
                value: value.as_ref(),
                expiration: ttl.into(),
                version: RecordVersion::now(),
            },
            keyspace_version: None,
        })
        .await
    }

    pub async fn del(&self, namespace: Namespace, key: impl AsRef<[u8]>) -> Result<(), Error> {
        self.execute_request(op::DelBorrowed {
            namespace,
            key: key.as_ref(),
            version: RecordVersion::now(),
            keyspace_version: None,
        })
        .await
    }

    pub async fn get_exp(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<Duration>, Error> {
        Ok(self
            .execute_request::<Option<RecordExpiration>>(op::GetExpBorrowed {
                namespace,
                key: key.as_ref(),
                keyspace_version: None,
            })
            .await?
            .map(Into::into))
    }

    pub async fn set_exp(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
        ttl: impl Into<RecordExpiration>,
    ) -> Result<(), Error> {
        self.execute_request(op::SetExpBorrowed {
            namespace,
            key: key.as_ref(),
            expiration: ttl.into(),
            version: RecordVersion::now(),
            keyspace_version: None,
        })
        .await
    }

    pub async fn hget(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
        field: impl AsRef<[u8]>,
    ) -> Result<Option<Record>, Error> {
        self.execute_request::<Option<Record>>(op::HGetBorrowed {
            namespace,
            key: key.as_ref(),
            field: field.as_ref(),
            keyspace_version: None,
        })
        .await
    }

    pub async fn hset(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
        field: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
        ttl: impl Into<RecordExpiration>,
    ) -> Result<(), Error> {
        self.execute_request(op::HSetBorrowed {
            namespace,
            key: key.as_ref(),
            entry: MapEntryBorrowed {
                field: field.as_ref(),
                record: RecordBorrowed {
                    value: value.as_ref(),
                    expiration: ttl.into(),
                    version: RecordVersion::now(),
                },
            },
            keyspace_version: None,
        })
        .await
    }

    pub async fn hdel(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
        field: impl AsRef<[u8]>,
    ) -> Result<(), Error> {
        self.execute_request(op::HDelBorrowed {
            namespace,
            key: key.as_ref(),
            field: field.as_ref(),
            version: RecordVersion::now(),
            keyspace_version: None,
        })
        .await
    }

    pub async fn hget_exp(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
        field: impl AsRef<[u8]>,
    ) -> Result<Option<Duration>, Error> {
        Ok(self
            .execute_request::<Option<RecordExpiration>>(op::HGetExpBorrowed {
                namespace,
                key: key.as_ref(),
                field: field.as_ref(),
                keyspace_version: None,
            })
            .await?
            .map(Into::into))
    }

    pub async fn hset_exp(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
        field: impl AsRef<[u8]>,
        ttl: impl Into<RecordExpiration>,
    ) -> Result<(), Error> {
        self.execute_request(op::HSetExpBorrowed {
            namespace,
            key: key.as_ref(),
            field: field.as_ref(),
            expiration: ttl.into(),
            version: RecordVersion::now(),
            keyspace_version: None,
        })
        .await
    }

    pub async fn hcard(&self, namespace: Namespace, key: impl AsRef<[u8]>) -> Result<u64, Error> {
        self.execute_request(op::HCardBorrowed {
            namespace,
            key: key.as_ref(),
            keyspace_version: None,
        })
        .await
    }

    pub async fn hscan(
        &self,
        namespace: Namespace,
        key: impl AsRef<[u8]>,
        count: u32,
        cursor: Option<impl AsRef<[u8]>>,
    ) -> Result<MapPage, Error> {
        self.execute_request::<MapPage>(op::HScanBorrowed {
            namespace,
            key: key.as_ref(),
            count,
            cursor: cursor.as_ref().map(|cursor| cursor.as_ref()),
            keyspace_version: None,
        })
        .await
    }

    async fn execute_request<R>(&self, op: impl Into<op::Borrowed<'_>>) -> Result<R, Error>
    where
        op::Output: op::DowncastOutput<R>,
    {
        self.inner
            .execute(op::Operation::Borrowed(op.into()))
            .await?
            .try_into()
            .map_err(|_| Error::InvalidResponseType)
    }
}

#[derive(Debug, Clone, Copy, Ordinalize)]
pub enum OperationName {
    Get,
    Set,
    Del,
    GetExp,
    SetExp,
    HGet,
    HSet,
    HDel,
    HGetExp,
    HSetExp,
    HCard,
    HScan,
}

pub trait NodeData: Send + Sync + Clone + 'static {
    fn new(operator_id: &NodeOperatorId, node: &Node) -> Self;
}

pub trait RequestObserver {
    type NodeData: NodeData;

    fn observe(
        &self,
        node: &Self::NodeData,
        duration: Duration,
        operation: OperationName,
        result: &Result<OperationOutput, Error>,
    );
}

impl NodeData for () {
    fn new(_: &NodeOperatorId, _: &Node) -> Self {}
}

impl RequestObserver for () {
    type NodeData = ();

    fn observe(
        &self,
        _: &Self::NodeData,
        _: Duration,
        _: OperationName,
        _: &Result<OperationOutput, Error>,
    ) {
    }
}

impl metrics::Enum for OperationName {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Set => "set",
            Self::Del => "del",
            Self::GetExp => "get_exp",
            Self::SetExp => "set_exp",
            Self::HGet => "hget",
            Self::HSet => "hset",
            Self::HDel => "hdel",
            Self::HGetExp => "hget_exp",
            Self::HSetExp => "hset_exp",
            Self::HCard => "hcard",
            Self::HScan => "hscan",
        }
    }
}

#[derive(Clone, Copy)]
enum Route {
    Any,
    Private,
}

pub(crate) struct Connector<API: Api> {
    public_conn: Connection<API>,
    private_conn: Option<Connection<API>>,
}

impl<API: Api> Connector<API> {
    pub(crate) fn is_open(&self, route: Route) -> bool {
        match route {
            Route::Any => self.is_any_open(),
            Route::Private => self.is_private_open(),
        }
    }

    fn is_any_open(&self) -> bool {
        let public_open = !self.public_conn.is_closed();
        let private_open = self.is_private_open();

        public_open || private_open
    }

    fn is_private_open(&self) -> bool {
        self.private_conn
            .as_ref()
            .map(|conn| !conn.is_closed())
            .unwrap_or(false)
    }

    pub(crate) async fn wait_open(&self) -> &Connection<API> {
        // Prefer private connection if it's available.
        if let Some(conn) = &self.private_conn
            && !conn.is_closed()
        {
            return conn;
        }

        let public_fut = self.public_conn.wait_open().map(|_| &self.public_conn);

        if let Some(private) = &self.private_conn {
            let private_fut = private.wait_open().map(|_| private);

            (public_fut, private_fut).race().await
        } else {
            public_fut.await
        }
    }
}
