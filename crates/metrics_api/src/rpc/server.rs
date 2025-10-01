use {
    super::*,
    crate::rpc,
    wcn_rpc::{PeerId, server::PendingConnection},
};

impl<S> wcn_rpc::server::Api for MetricsApi<S>
where
    S: crate::Factory<PeerId>,
{
    type RpcHandler = RpcHandler<S::MetricsApi>;

    async fn handle_connection(conn: PendingConnection<'_, Self>) -> wcn_rpc::server::Result<()> {
        let metrics_api = match conn.api().state.new_metrics_api(*conn.remote_peer_id()) {
            Ok(api) => api,
            Err(err) => {
                return conn
                    .reject(ConnectionRejectionReason::from(err) as u8)
                    .await;
            }
        };

        let conn = conn.accept(RpcHandler { metrics_api }).await?;

        conn.handle(|rpc| async move {
            match rpc.id() {
                rpc::Id::GetMetrics => {
                    rpc.handle_request::<GetMetrics>(RpcHandler::get_metrics)
                        .await
                }
            }
        })
        .await
    }
}

#[derive(Clone)]
pub struct RpcHandler<M: crate::MetricsApi> {
    metrics_api: M,
}

impl<M: crate::MetricsApi> RpcHandler<M> {
    async fn get_metrics(self, path: String) -> rpc::Result<String> {
        self.metrics_api
            .get_metrics(&path)
            .await
            .map_err(Into::into)
    }
}
