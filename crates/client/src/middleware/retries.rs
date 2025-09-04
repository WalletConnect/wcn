use crate::{
    Client,
    ClientBuilder,
    Config,
    CoordinatorErrorKind,
    Error,
    LoadBalancer,
    MiddlewareBuilder,
    NodeData,
    OperationOutput,
    Request,
    RequestExecutor,
    cluster,
};

pub struct WithRetriesBuilder<T> {
    inner: T,
    max_attempts: usize,
}

impl<T> MiddlewareBuilder for WithRetriesBuilder<T>
where
    T: MiddlewareBuilder + Send + Sync,
{
    type NodeData = T::NodeData;

    type Inner<D>
        = WithRetries<T::Inner<D>>
    where
        D: NodeData;

    async fn build<D>(self, config: Config) -> Result<Self::Inner<D>, Error>
    where
        D: NodeData,
    {
        Ok(WithRetries {
            core: self.inner.build(config).await?,
            max_attempts: self.max_attempts,
        })
    }
}

pub struct WithRetries<C = Client> {
    core: C,
    max_attempts: usize,
}

impl<C, D> LoadBalancer<D> for WithRetries<C>
where
    C: LoadBalancer<D> + Send + Sync,
{
    fn using_next_node<F, R>(&self, cb: F) -> R
    where
        F: Fn(&cluster::Node<D>) -> R,
    {
        self.core.using_next_node(cb)
    }
}

impl<C> RequestExecutor for WithRetries<C>
where
    C: RequestExecutor + Send + Sync,
{
    async fn execute(&self, req: Request<'_>) -> Result<OperationOutput, Error> {
        let mut attempt = 0;

        while attempt < self.max_attempts {
            match self.core.execute(req.clone()).await {
                Ok(data) => return Ok(data),

                Err(err) => match err {
                    Error::CoordinatorApi(err)
                        if err.kind() == CoordinatorErrorKind::Timeout
                            || err.kind() == CoordinatorErrorKind::Transport =>
                    {
                        attempt += 1
                    }

                    err => return Err(err),
                },
            }
        }

        Err(Error::RetriesExhausted)
    }
}

impl<T> ClientBuilder<T>
where
    T: MiddlewareBuilder,
{
    pub fn with_retries(self, max_attempts: usize) -> ClientBuilder<WithRetriesBuilder<T>> {
        ClientBuilder {
            inner: WithRetriesBuilder {
                inner: self.inner,
                max_attempts,
            },
            config: self.config,
        }
    }
}
