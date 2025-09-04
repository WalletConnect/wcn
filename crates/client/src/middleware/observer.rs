use {
    crate::{
        Client,
        ClientBuilder,
        Config,
        Error,
        LoadBalancer,
        MiddlewareBuilder,
        NodeData,
        NodeOperatorId,
        OperationOutput,
        PeerId,
        Request,
        RequestExecutor,
        cluster,
    },
    std::time::{Duration, Instant},
    wc::metrics::{self, enum_ordinalize::Ordinalize},
    wcn_storage_api::operation as op,
};

#[derive(Debug, Clone)]
pub struct RequestMetadata<D = ()> {
    pub operator_id: NodeOperatorId,
    pub node_id: PeerId,
    pub node_data: D,
    pub operation: OperationName,
    pub duration: Duration,
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

pub trait RequestObserver {
    type NodeData: NodeData;

    fn observe(
        &self,
        metadata: RequestMetadata<Self::NodeData>,
        result: &Result<OperationOutput, Error>,
    );
}

pub struct WithObserverBuilder<O, T> {
    inner: T,
    observer: O,
}

impl<O, T> MiddlewareBuilder for WithObserverBuilder<O, T>
where
    O: RequestObserver + Send + Sync,
    T: MiddlewareBuilder + Send + Sync,
{
    type NodeData = O::NodeData;

    type Inner<D>
        = WithObserver<O, T::Inner<D>>
    where
        D: NodeData;

    async fn build<D>(self, config: Config) -> Result<Self::Inner<D>, Error>
    where
        D: NodeData,
    {
        Ok(WithObserver {
            core: self.inner.build(config).await?,
            observer: self.observer,
        })
    }
}

pub struct WithObserver<O: RequestObserver, C = Client> {
    core: C,
    observer: O,
}

impl<O, C, D> LoadBalancer<D> for WithObserver<O, C>
where
    O: RequestObserver<NodeData = D>,
    C: LoadBalancer<D> + Send + Sync,
{
    fn using_next_node<F, R>(&self, cb: F) -> R
    where
        F: Fn(&cluster::Node<D>) -> R,
    {
        self.core.using_next_node(cb)
    }
}

impl<O, C, D> RequestExecutor for WithObserver<O, C>
where
    O: RequestObserver<NodeData = D> + Send + Sync,
    C: RequestExecutor + LoadBalancer<D> + Send + Sync,
    D: Send + Sync + Clone,
{
    async fn execute(&self, req: Request<'_>) -> Result<OperationOutput, Error> {
        let op_name = op_name(&req.op);
        let start_time = Instant::now();

        let (conn, mut metadata) = self.core.using_next_node(|node| {
            (node.coordinator_conn.clone(), RequestMetadata {
                operator_id: node.operator_id,
                node_id: node.node.peer_id,
                node_data: node.data.clone(),
                operation: op_name,
                duration: Duration::ZERO,
            })
        });

        let result = self.core.execute(req.with_connection(conn)).await;
        metadata.duration = start_time.elapsed();
        self.observer.observe(metadata, &result);

        result
    }
}

impl<T> ClientBuilder<T>
where
    T: MiddlewareBuilder,
{
    pub fn with_observer<O>(self, observer: O) -> ClientBuilder<WithObserverBuilder<O, T>>
    where
        O: RequestObserver,
    {
        ClientBuilder {
            inner: WithObserverBuilder {
                inner: self.inner,
                observer,
            },
            config: self.config,
        }
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

fn op_name(op: &op::Operation<'_>) -> OperationName {
    match op {
        op::Operation::Owned(op) => match op {
            op::Owned::Get(_) => OperationName::Get,
            op::Owned::Set(_) => OperationName::Set,
            op::Owned::Del(_) => OperationName::Del,
            op::Owned::GetExp(_) => OperationName::GetExp,
            op::Owned::SetExp(_) => OperationName::SetExp,
            op::Owned::HGet(_) => OperationName::HGet,
            op::Owned::HSet(_) => OperationName::HSet,
            op::Owned::HDel(_) => OperationName::HDel,
            op::Owned::HGetExp(_) => OperationName::HGetExp,
            op::Owned::HSetExp(_) => OperationName::HSetExp,
            op::Owned::HCard(_) => OperationName::HCard,
            op::Owned::HScan(_) => OperationName::HScan,
        },

        op::Operation::Borrowed(op) => match op {
            op::Borrowed::Get(_) => OperationName::Get,
            op::Borrowed::Set(_) => OperationName::Set,
            op::Borrowed::Del(_) => OperationName::Del,
            op::Borrowed::GetExp(_) => OperationName::GetExp,
            op::Borrowed::SetExp(_) => OperationName::SetExp,
            op::Borrowed::HGet(_) => OperationName::HGet,
            op::Borrowed::HSet(_) => OperationName::HSet,
            op::Borrowed::HDel(_) => OperationName::HDel,
            op::Borrowed::HGetExp(_) => OperationName::HGetExp,
            op::Borrowed::HSetExp(_) => OperationName::HSetExp,
            op::Borrowed::HCard(_) => OperationName::HCard,
            op::Borrowed::HScan(_) => OperationName::HScan,
        },
    }
}
