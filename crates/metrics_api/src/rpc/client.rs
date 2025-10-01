use super::*;

/// [`wcn_rpc::Client`] of [`MetricsApi`].
pub type Client = wcn_rpc::Client<MetricsApi>;

/// Outbound [`wcn_rpc::client::Connection`] to [`MetricsApi`].
pub type Connection = wcn_rpc::client::Connection<MetricsApi>;

impl wcn_rpc::client::Api for MetricsApi {
    type ConnectionParameters = ();
}

impl crate::MetricsApi for Connection {
    async fn get_metrics(&self, path: &str) -> crate::Result<String> {
        Ok(GetMetrics::send_request(self, path).await??)
    }
}

impl From<wcn_rpc::client::Error> for crate::Error {
    fn from(err: wcn_rpc::client::Error) -> Self {
        Self::new(crate::ErrorKind::Transport)
            .with_message(format!("wcn_metrics_api::client::Error: {err}"))
    }
}
