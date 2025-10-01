use {
    crate::{Target, TargetInner},
    derive_where::derive_where,
    std::{collections::HashMap, sync::Arc},
    tap::Pipe as _,
    wcn_cluster::PeerId,
    wcn_metrics_api::MetricsApi,
};

/// Metrics provider.
#[derive(Clone)]
pub struct Provider<M: MetricsApi, A: Authorization = sealed::NoAuth> {
    targets: Arc<HashMap<&'static str, Target<M>>>,
    auth: A,
}

/// [`Provider`] with [local targets](`Target::local`) only.
pub type LocalProvider<A = sealed::NoAuth> = Provider<super::sealed::NoRemote, A>;

impl<M: MetricsApi> Provider<M> {
    /// Creates a new [`Provider`].
    pub fn new<const N: usize>(targets: [Target<M>; N]) -> Self {
        targets
            .into_iter()
            .map(|target| (target.name, target))
            .collect::<HashMap<_, _>>()
            .pipe(Arc::new)
            .pipe(|targets| Self {
                targets,
                auth: sealed::NoAuth,
            })
    }

    /// Adds [`ProviderAuthorization`] to this [`Provider`].
    pub fn with_auth<A: Authorization>(self, auth: A) -> Provider<M, A> {
        Provider {
            targets: self.targets,
            auth,
        }
    }
}

impl<M: MetricsApi, A: Authorization> Provider<M, A> {
    /// Establishes a new [`InboundConnection`].
    ///
    /// Returns `None` if the peer is not authorized to use this
    /// [`Provider`].
    pub fn new_inbound_connection(&self, peer_id: PeerId) -> Option<InboundConnection<M, A>> {
        if !self.auth.is_authorized(&peer_id) {
            return None;
        }

        Some(InboundConnection {
            provider: self.clone(),
        })
    }
}

/// Inbound connection to the local [`Provider`] from a remote peer.
#[derive_where(Clone)]
pub struct InboundConnection<M: MetricsApi, A: Authorization> {
    provider: Provider<M, A>,
}

impl<M: MetricsApi, A: Authorization> wcn_metrics_api::Factory<PeerId> for Provider<M, A> {
    type MetricsApi = InboundConnection<M, A>;

    fn new_metrics_api(&self, peer_id: PeerId) -> wcn_metrics_api::FactoryResult<Self::MetricsApi> {
        self.new_inbound_connection(peer_id)
            .ok_or_else(wcn_metrics_api::FactoryError::unauthorized)
    }
}

impl<M: MetricsApi, A: Authorization> MetricsApi for InboundConnection<M, A> {
    async fn get_metrics(&self, target_name: &str) -> wcn_metrics_api::Result<String> {
        let target = self
            .provider
            .targets
            .get(target_name)
            .ok_or_else(wcn_metrics_api::Error::not_found)?;

        match &target.inner {
            TargetInner::Local(prometheus_handle) => Ok(prometheus_handle.render()),
            TargetInner::Remote(metrics_api) => metrics_api.get_metrics(target_name).await,
        }
    }
}

mod sealed {
    #[derive(Clone)]
    pub struct NoAuth;
}

/// [`Provider`] authorization mechanism.
pub trait Authorization: Clone + Send + Sync + 'static {
    /// Indicates whether the specified peer is authorized to use the
    /// [`Provider`].
    fn is_authorized(&self, peer_id: &PeerId) -> bool;
}

impl Authorization for sealed::NoAuth {
    fn is_authorized(&self, _peer_id: &PeerId) -> bool {
        true
    }
}

#[cfg(feature = "cluster")]
impl<C> Authorization for wcn_cluster::Cluster<C>
where
    C: wcn_cluster::Config,
{
    fn is_authorized(&self, peer_id: &PeerId) -> bool {
        self.contains_node(peer_id)
    }
}
