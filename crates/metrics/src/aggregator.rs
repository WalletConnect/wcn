use {
    derive_where::derive_where,
    std::sync::Arc,
    wcn_cluster::{Cluster, PeerId, node_operator},
    wcn_metrics_api::MetricsApi,
};

/// [`Aggregator`] config.
pub trait Config: wcn_cluster::Config {
    /// Type of the outbound [`MetricsApi`] connection.
    type OutboundMetricsApiConnection: MetricsApi;
}

#[derive_where(Clone)]
pub struct Aggregator<C: Config> {
    _config: Arc<C>,
    cluster: Cluster<C>,
}

impl<C> Aggregator<C>
where
    C: Config<Node: AsRef<C::OutboundMetricsApiConnection>>,
{
    /// Creates a new metrics [`Aggregator`].
    pub fn new(config: C, cluster: Cluster<C>) -> Self {
        Self {
            _config: Arc::new(config),
            cluster,
        }
    }

    pub async fn render(&self) -> String {
        let cluster_view = self.cluster.view();

        cluster_view
            .node_operators()
            .iter()
            .map(|operator| operator.nodes())
    }

    async fn render_node(operator: node_operator::Id, node_idx: usize, node: &C::Node) -> String {
        prometheus_parse::Scrape::parse();

        conn.get_metrics("node")
            .await
            .map_err(|err| tracing::warn!(%operator, %node_idx, ?err, "Failed fetch Node metrics"))
            .unwrap_or_default()
    }

    async fn get_node_metrics(node: &C::Node) -> wcn_metrics_api::Result<String> {
        let conn: &C::OutboundMetricsApiConnection = node.as_ref();
        conn.get_metrics("node").await
    }
}
