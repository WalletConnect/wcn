use {
    crate::{
        keyspace,
        smart_contract::{self, Read as _},
        view,
        Config,
        Inner,
        Keyspace,
    },
    futures::{Stream, StreamExt},
    std::{pin::pin, sync::Arc, time::Duration},
    tokio::sync::watch,
    tracing::Instrument,
};

pub(super) struct Task<C: Config, Events> {
    pub initial_events: Option<Events>,
    pub inner: Arc<Inner<C>>,
    pub watch: watch::Sender<()>,
}

pub(super) struct Guard(tokio::task::JoinHandle<()>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.abort();
        tracing::info!("aborted");
    }
}

// Emits the cluster and keyspace version metrics
fn cluster_keyspace_metrics<C: Config>(view: &crate::View<C>) {
    wc::metrics::gauge!("wcn_node_cluster_version")
        .set(u32::try_from(view.cluster_version).unwrap_or(u32::MAX));

    wc::metrics::gauge!("wcn_node_keyspace_version")
        .set(u32::try_from(view.keyspace_version).unwrap_or(u32::MAX));
}

impl<C: Config, Events> Task<C, Events>
where
    Events: Stream<Item = smart_contract::ReadResult<smart_contract::Event>> + Send + 'static,
    Keyspace: keyspace::sealed::Calculate<C::KeyspaceShards>,
{
    pub(super) fn spawn(self) -> Guard {
        let guard = Guard(tokio::spawn(self.run().in_current_span()));
        tracing::info!("spawned");
        guard
    }

    async fn run(mut self) {
        // apply initial events until they finish / first error
        if let Some(events) = self.initial_events.take() {
            match self.apply_events(events).await {
                Ok(()) => tracing::warn!("Initial event stream finished"),
                Err(err) => tracing::error!(%err, "Failed to apply initial events"),
            }
        }

        loop {
            // when we fail for whatever reason - subscribe again and refetch the whole
            // state
            match self.update_view().await {
                Ok(()) => tracing::warn!("Event stream finished"),
                Err(err) => {
                    tracing::error!(%err, "Failed to update cluster::View");
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            }
        }
    }

    async fn update_view(&mut self) -> Result<()> {
        let events = self.inner.smart_contract.events().await?;

        let new_view = self.inner.smart_contract.cluster_view().await?;
        if self.inner.view.load().cluster_version != new_view.cluster_version {
            let new_view = Arc::new(crate::View::try_from_sc(new_view, &self.inner.config).await?);
            cluster_keyspace_metrics(&new_view);
            self.inner.view.store(new_view);
            let _ = self.watch.send(());
        }

        self.apply_events(events).await
    }

    #[allow(clippy::needless_pass_by_ref_mut)] // otherwise `Steam` is required to be `Sync`
    async fn apply_events(
        &mut self,
        events: impl Stream<Item = smart_contract::ReadResult<smart_contract::Event>>,
    ) -> Result<()> {
        let mut events = pin!(events);

        while let Some(res) = events.next().await {
            let event = res?;
            tracing::info!(?event, "received");

            let cfg = &self.inner.config;
            let view = self.inner.view.load_full();
            let view = Arc::new((*view).clone().apply_event(cfg, event).await?);
            cluster_keyspace_metrics(&view);
            self.inner.view.store(view);
            let _ = self.watch.send(());
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    ApplyEvent(#[from] view::Error),

    #[error(transparent)]
    InvalidClusterView(#[from] view::TryFromSmartContractError),

    #[error(transparent)]
    SmartContractRead(#[from] smart_contract::ReadError),
}

type Result<T, E = Error> = std::result::Result<T, E>;
