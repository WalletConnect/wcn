use {
    crate::{
        Config,
        CoordinatorConnection,
        Error,
        LoadBalancer,
        NodeData,
        Request,
        RequestExecutor,
        cluster,
    },
    arc_swap::ArcSwap,
    std::{sync::Arc, time::Duration},
    tokio::sync::oneshot,
    wc::future::FutureExt,
    wcn_storage_api::{StorageApi, operation as op},
};

pub struct Client<D: NodeData = ()> {
    cluster: cluster::Cluster<D>,
    connection_timeout: Duration,
    _shutdown_tx: oneshot::Sender<()>,
}

impl<D> Client<D>
where
    D: NodeData,
{
    pub(crate) async fn new(config: Config) -> Result<Self, Error> {
        let cluster_api =
            wcn_cluster_api::rpc::ClusterApi::new().with_rpc_timeout(Duration::from_secs(5));

        let cluster_api_client_cfg = wcn_rpc::client::Config {
            keypair: config.keypair.clone(),
            connection_timeout: config.connection_timeout,
            reconnect_interval: config.reconnect_interval,
            max_concurrent_rpcs: 50,
            max_idle_connection_timeout: config.max_idle_connection_timeout,
            priority: wcn_rpc::transport::Priority::High,
        };

        let cluster_api_client = wcn_rpc::client::Client::new(cluster_api_client_cfg, cluster_api)?;

        let coordinator_api =
            wcn_storage_api::rpc::CoordinatorApi::new().with_rpc_timeout(Duration::from_secs(2));

        let coordinator_api_client_cfg = wcn_rpc::client::Config {
            keypair: config.keypair,
            connection_timeout: config.connection_timeout,
            reconnect_interval: config.reconnect_interval,
            max_concurrent_rpcs: config.max_concurrent_rpcs,
            max_idle_connection_timeout: config.max_idle_connection_timeout,
            priority: wcn_rpc::transport::Priority::High,
        };

        let coordinator_api_client =
            wcn_rpc::client::Client::new(coordinator_api_client_cfg, coordinator_api)?;

        // Initialize the client using one or more bootstrap nodes:
        // - fetch the current version of the cluster view;
        // - using the cluster view, initialize [`wcn_cluster::Cluster`];
        // - from a properly initialized [`wcn_cluster::Cluster`] obtain a
        //   [`wcn_cluster::View`];
        // - use the view to create a different version of [`cluster::SmartContract`],
        //   which would always use an up-to-date version of the cluster view for the
        //   cluster API;
        // - spawn a task to monitor cluster updates and send an up-to-date version of
        //   the cluster view to the smart contract.
        let initial_cluster_view =
            cluster::fetch_cluster_view(&cluster_api_client, &config.nodes).await?;

        let cluster_cfg = cluster::Config::new(
            config.cluster_key,
            cluster_api_client,
            coordinator_api_client,
        );

        let bootstrap_sc = cluster::SmartContract::Static(initial_cluster_view);
        let bootstrap_cluster = cluster::Cluster::new(cluster_cfg.clone(), bootstrap_sc).await?;
        let cluster_view = Arc::new(ArcSwap::new(bootstrap_cluster.view()));
        let dynamic_sc = cluster::SmartContract::Dynamic(cluster_view.clone());
        let cluster = cluster::Cluster::new(cluster_cfg, dynamic_sc).await?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        tokio::spawn(cluster::update_task(
            shutdown_rx,
            cluster.clone(),
            cluster_view,
        ));

        Ok(Self {
            cluster,
            connection_timeout: config.connection_timeout,
            _shutdown_tx: shutdown_tx,
        })
    }

    async fn execute_request(
        &self,
        client_conn: &CoordinatorConnection,
        op: &op::Operation<'_>,
    ) -> Result<op::Output, Error> {
        let is_connected = client_conn
            .wait_open()
            .with_timeout(self.connection_timeout)
            .await
            .is_ok();

        if !is_connected {
            // Getting to this point means we've tried every operator to find a connected
            // node and failed. Then we tried to open connection to the next node and also
            // failed.
            return Err(Error::NoAvailableNodes);
        }

        client_conn.execute_ref(op).await.map_err(Into::into)
    }
}

impl<D> LoadBalancer<D> for Client<D>
where
    D: NodeData,
{
    fn using_next_node<F, R>(&self, cb: F) -> R
    where
        F: Fn(&cluster::Node<D>) -> R,
    {
        // Constraints:
        // - Each next request should go to a different operator.
        // - Find an available (i.e. connected) node of the next operator, filtering out
        //   broken connections. The expectation is that each operator should have at
        //   least one available node at all times.

        self.cluster.using_view(|view| {
            let operators = view.node_operators();

            // Iterate over all of the operators to find one with a connected node.
            let result = operators.find_next_operator(|operator| {
                operator
                    .find_next_node(|node| (!node.coordinator_conn.is_closed()).then(|| cb(node)))
            });

            if let Some(result) = result {
                // We've found a connected node.
                result
            } else {
                // If the above failed, return the next node in hopes that the connection will
                // be established during the request.
                cb(operators.next().next_node())
            }
        })
    }
}

impl<D> RequestExecutor for Client<D>
where
    D: NodeData,
{
    async fn execute(&self, req: Request<'_>) -> Result<op::Output, Error> {
        let op = req.op;
        let conn = req
            .conn
            .unwrap_or_else(|| self.using_next_node(|node| node.coordinator_conn.clone()));

        self.execute_request(&conn, &op).await
    }
}
