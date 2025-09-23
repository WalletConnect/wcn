use {
    crate::{
        Config,
        EncryptionKey,
        Error,
        OperationName,
        RequestObserver,
        cluster,
        encryption::{self, Encrypt as _},
    },
    arc_swap::ArcSwap,
    std::{
        sync::Arc,
        time::{Duration, Instant},
    },
    tokio::sync::oneshot,
    wc::future::FutureExt,
    wcn_storage_api::{StorageApi, operation as op, rpc::client::CoordinatorConnection},
};

pub struct BaseClient<T: RequestObserver> {
    cluster: cluster::Cluster<T::NodeData>,
    connection_timeout: Duration,
    max_attempts: usize,
    observer: T,
    encryption_key: Option<EncryptionKey>,
    _shutdown_tx: oneshot::Sender<()>,
}

impl<T> BaseClient<T>
where
    T: RequestObserver,
{
    pub async fn new(
        config: Config,
        observer: T,
        encryption_key: Option<EncryptionKey>,
    ) -> Result<Self, Error> {
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

        // Initialize the client using one or more nodes:
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
            max_attempts: config.max_retries + 1,
            observer,
            encryption_key,
            _shutdown_tx: shutdown_tx,
        })
    }

    pub async fn execute(&self, op: op::Operation<'_>) -> Result<op::Output, Error> {
        let op = if let Some(key) = &self.encryption_key {
            op.encrypt(key)?
        } else {
            op
        };

        if self.max_attempts > 1 {
            let mut attempt = 0;

            while attempt < self.max_attempts {
                match self.execute_internal(&op).await {
                    Ok(data) => return Ok(data),

                    Err(err) => match err {
                        Error::CoordinatorApi(err) if err.is_transient() => attempt += 1,
                        err => return Err(err),
                    },
                }
            }

            Err(Error::RetriesExhausted)
        } else {
            self.execute_internal(&op).await
        }
    }

    async fn execute_internal(&self, op: &op::Operation<'_>) -> Result<op::Output, Error> {
        let (conn, node_data) = self.find_next_node();

        let is_connected = conn
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

        let start_time = Instant::now();

        let result = async {
            let mut output = conn.execute_ref(op).await.map_err(Error::from)?;

            if let Some(key) = &self.encryption_key {
                encryption::decrypt_output(&mut output, key)?;
            }

            Ok(output)
        }
        .await;

        self.observer
            .observe(&node_data, start_time.elapsed(), op_name(op), &result);

        result
    }

    fn find_next_node(&self) -> (CoordinatorConnection, T::NodeData) {
        // Constraints:
        // - Each next request should go to a different operator.
        // - Find an available (i.e. connected) node of the next operator, filtering out
        //   broken connections. The expectation is that each operator should have at
        //   least one available node at all times.

        self.cluster.using_view(|view| {
            let operators = view.node_operators();

            // Iterate over all of the operators to find one with a connected node.
            let result = operators.find_next_operator(|operator| {
                operator.find_next_node(|node| {
                    (!node.coordinator_conn.is_closed())
                        .then(|| (node.coordinator_conn.clone(), node.data.clone()))
                })
            });

            if let Some(result) = result {
                // We've found a connected node.
                result
            } else {
                // If the above failed, return the next node in hopes that the connection will
                // be established during the request.
                let node = operators.next().next_node();

                (node.coordinator_conn.clone(), node.data.clone())
            }
        })
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
