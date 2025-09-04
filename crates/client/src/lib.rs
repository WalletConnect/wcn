use {
    crate::middleware::MiddlewareBuilder,
    derive_where::derive_where,
    sealed::{LoadBalancer, Request},
    std::{net::SocketAddrV4, sync::Arc, time::Duration},
    wcn_cluster::smart_contract,
    wcn_storage_api::{
        MapEntryBorrowed,
        Record,
        RecordBorrowed,
        RecordExpiration,
        RecordVersion,
        operation as op,
        rpc::client::CoordinatorConnection,
    },
};
pub use {
    client::Client,
    libp2p_identity::{Keypair, PeerId},
    middleware::{
        BaseClientBuilder,
        EncryptionError,
        EncryptionKey,
        OperationName,
        RequestMetadata,
        RequestObserver,
        WithEncryption,
        WithEncryptionBuilder,
        WithObserver,
        WithObserverBuilder,
        WithRetries,
        WithRetriesBuilder,
    },
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
mod middleware;

#[derive(Debug, thiserror::Error)]
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

    /// [`PeerAddr`]s of the bootstrap nodes.
    pub nodes: Vec<PeerAddr>,

    /// Additional metrics tag being used for all [`Driver`] metrics.
    pub metrics_tag: &'static str,
}

#[derive(Debug, Clone)]
pub struct PeerAddr {
    pub id: PeerId,
    pub addr: SocketAddrV4,
}

pub struct ClientBuilder<T> {
    inner: T,
    config: Config,
}

impl ClientBuilder<BaseClientBuilder> {
    pub fn new(config: Config) -> Self {
        Self {
            inner: BaseClientBuilder {},
            config,
        }
    }
}

impl<T> ClientBuilder<T>
where
    T: MiddlewareBuilder,
{
    pub async fn build(self) -> Result<CoordinatorClient<T::Inner<T::NodeData>>, Error> where {
        let inner = self.inner.build(self.config).await?;

        Ok(CoordinatorClient {
            inner: Arc::new(inner),
        })
    }
}

pub trait NodeData: Send + Sync + Clone + 'static {
    fn init(operator_id: &NodeOperatorId, node: &Node) -> Self;
}

impl NodeData for () {
    fn init(_: &NodeOperatorId, _: &Node) -> Self {}
}

pub trait RequestExecutor {
    fn execute(
        &self,
        req: Request<'_>,
    ) -> impl Future<Output = Result<op::Output, Error>> + Send + Sync;
}

#[derive_where(Clone)]
pub struct CoordinatorClient<C> {
    inner: Arc<C>,
}

impl<C> CoordinatorClient<C>
where
    C: RequestExecutor + Send + Sync,
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
        ttl: Duration,
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
        ttl: Duration,
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
        ttl: Duration,
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
        ttl: Duration,
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

    async fn execute_request<T>(&self, op: impl Into<op::Borrowed<'_>>) -> Result<T, Error>
    where
        op::Output: op::DowncastOutput<T>,
    {
        self.inner
            .execute(Request::new(op::Operation::Borrowed(op.into())))
            .await?
            .try_into()
            .map_err(|_| Error::InvalidResponseType)
    }
}

mod sealed {
    use super::*;

    pub trait LoadBalancer<D> {
        fn using_next_node<F, R>(&self, cb: F) -> R
        where
            F: Fn(&cluster::Node<D>) -> R;
    }

    #[derive(Clone)]
    pub struct Request<'a> {
        pub op: op::Operation<'a>,
        pub conn: Option<CoordinatorConnection>,
    }

    impl<'a> Request<'a> {
        pub fn new(op: op::Operation<'a>) -> Self {
            Self { op, conn: None }
        }

        pub fn with_connection(self, conn: CoordinatorConnection) -> Self {
            Self {
                op: self.op,
                conn: Some(conn),
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[allow(dead_code)]
    #[derive(Clone)]
    struct TestData {
        foo: u32,
    }

    impl NodeData for TestData {
        fn init(_: &NodeOperatorId, _: &Node) -> Self {
            Self { foo: 42 }
        }
    }

    #[allow(dead_code)]
    struct Observer;

    impl RequestObserver for Observer {
        type NodeData = TestData;

        fn observe(
            &self,
            metadata: RequestMetadata<Self::NodeData>,
            _result: &Result<op::Output, Error>,
        ) {
            assert_eq!(metadata.node_data.foo, 42);
        }
    }

    async fn _create_raw_client(config: Config) -> CoordinatorClient<Client> {
        ClientBuilder::new(config).build().await.unwrap()
    }

    async fn _create_observable_client(
        config: Config,
    ) -> CoordinatorClient<WithEncryption<WithRetries<WithObserver<Observer, Client<TestData>>>>>
    {
        ClientBuilder::new(config)
            .with_observer(Observer)
            .with_retries(3)
            .with_encryption(EncryptionKey::new(b"12345").unwrap())
            .build()
            .await
            .unwrap()
    }

    async fn _create_retryable_client(config: Config) -> CoordinatorClient<WithRetries> {
        ClientBuilder::new(config)
            .with_retries(3)
            .build()
            .await
            .unwrap()
    }

    async fn _create_encrypted_client(config: Config) -> CoordinatorClient<WithEncryption> {
        ClientBuilder::new(config)
            .with_encryption(EncryptionKey::new(b"12345").unwrap())
            .build()
            .await
            .unwrap()
    }
}
