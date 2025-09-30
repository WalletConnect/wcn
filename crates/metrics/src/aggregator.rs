use {
    derive_where::derive_where,
    std::sync::Arc,
    wcn_cluster::{Cluster, PeerId},
    wcn_metrics_api::MetricsApi,
};

/// [`Aggregator`] config.
pub trait Config: wcn_cluster::Config {
    /// Type of the outbound [`MetricsApi`] connection.
    type OutboundConnection: MetricsApi + Clone;
}

#[derive_where(Clone)]
pub struct Aggregator<C: Config> {
    _config: Arc<C>,
    cluster: Cluster<C>,
}

impl<C: Config> Aggregator<C> {
    /// Creates a new metrics [`Aggregator`].
    pub fn new(config: C, cluster: Cluster<C>) -> Self {
        Self {
            _config: Arc::new(config),
            cluster,
        }
    }
}
