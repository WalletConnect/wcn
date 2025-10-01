use {wc::metrics::exporter_prometheus::PrometheusHandle, wcn_metrics_api::MetricsApi};

// #[cfg(feature = "cluster")]
// pub mod aggregator;
// #[cfg(feature = "cluster")]
// pub use aggregator::Aggregator;

pub mod provider;
pub use provider::{LocalProvider, Provider};

pub mod system;

/// Metrics target.
///
/// Represents a destinct service the metrics can be collected for.
pub struct Target<M: MetricsApi = sealed::NoRemote> {
    name: &'static str,
    inner: TargetInner<M>,
}

enum TargetInner<M: MetricsApi> {
    Local(PrometheusHandle),
    Remote(M),
}

impl<M: MetricsApi> Target<M> {
    /// Creates a new local [`Target`].
    ///
    /// Local [`Target`]s are the [`Target`]s located within the same process as
    /// [`Provider`].
    pub fn local(name: &'static str, prometheus_handle: PrometheusHandle) -> Self {
        Self {
            name,
            inner: TargetInner::Local(prometheus_handle),
        }
    }

    /// Creates a new remote [`Target`].
    ///
    /// Remote [`Target`]s are accessible over [`MetricsApi`].
    pub fn remote(name: &'static str, metrics_api: M) -> Self {
        Self {
            name,
            inner: TargetInner::Remote(metrics_api),
        }
    }
}

mod sealed {
    #[derive(Clone)]
    pub enum NoRemote {}
}

impl MetricsApi for sealed::NoRemote {
    async fn get_metrics(&self, _target: &str) -> wcn_metrics_api::Result<String> {
        match *self {}
    }
}
