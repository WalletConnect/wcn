use {
    derive_where::derive_where,
    futures::{FutureExt as _, Stream, StreamExt, TryFutureExt as _, stream},
    futures_concurrency::stream::StreamExt as _,
    std::fmt::{Display, Write as _},
    wcn_cluster::{Cluster, NodeOperator, node_operator},
    wcn_metrics_api::MetricsApi,
};

/// [`Aggregator`] config.
pub trait Config: wcn_cluster::Config {
    /// Returns [`Self::OutboundMetricsApiConnection`] to the provided [`Node`].
    fn outbound_metrics_api_connection(node: &Self::Node) -> &impl MetricsApi;
}

#[derive_where(Clone)]
pub struct Aggregator<C: Config> {
    cluster: Cluster<C>,
}

impl<C: Config> Aggregator<C> {
    /// Creates a new metrics [`Aggregator`].
    pub fn new(cluster: Cluster<C>) -> Self {
        Self { cluster }
    }

    /// Gets metrics from all of the nodes/databases within the cluster,
    /// aggregates them and renders into a final [`String`].
    ///
    /// Metrics are being streamed in chunks, each chunk represents a separate
    /// "service" (node / database) of a node operator.
    pub fn render_cluster_metrics(&self) -> impl Stream<Item = String> + Send + 'static {
        stream::iter(self.cluster.view().node_operators().clone().into_iter())
            .flat_map(move |operator| Self::render_operator_metrics(operator))
    }

    fn render_operator_metrics(
        operator: NodeOperator<C::Node>,
    ) -> impl Stream<Item = String> + Send + 'static {
        let nodes_count = operator.nodes().len();
        let operator_name = operator.name.clone();

        stream::iter(operator.nodes().to_vec().into_iter().enumerate())
            .map(move |(idx, node)| Self::render_node_metrics(operator_name.clone(), idx, node))
            .buffer_unordered(nodes_count)
            .merge(stream::once(Self::render_db_metrics(operator)))
    }

    async fn render_node_metrics(
        operator: node_operator::Name,
        node_idx: usize,
        node: C::Node,
    ) -> String {
        C::outbound_metrics_api_connection(&node)
            .get_metrics("node")
            .and_then(|metrics| {
                append_extra_labels(metrics, ExtraLabels::node(&operator, node_idx)).map(Ok)
            })
            .await
            .map_err(|err| tracing::warn!(%operator, %node_idx, ?err, "Failed to get Node metrics"))
            .unwrap_or_default()
    }

    async fn render_db_metrics(operator: NodeOperator<C::Node>) -> String {
        for node in operator.nodes_lb_iter() {
            let res = C::outbound_metrics_api_connection(node)
                .get_metrics("db")
                .await;

            match res {
                Ok(metrics) => {
                    return append_extra_labels(metrics, ExtraLabels::database(&operator.name))
                        .await;
                }
                Err(err) => {
                    tracing::warn!(operator= %operator.name.as_str(), ?err, "Failed to get Database metrics");
                }
            }
        }

        String::new()
    }
}

struct ExtraLabels<'a> {
    operator: &'a node_operator::Name,
    service_kind: ServiceKind,
}

impl<'a> ExtraLabels<'a> {
    fn database(operator: &'a node_operator::Name) -> Self {
        Self {
            operator,
            service_kind: ServiceKind::Database,
        }
    }

    fn node(operator: &'a node_operator::Name, node_idx: usize) -> Self {
        Self {
            operator,
            service_kind: ServiceKind::Node(node_idx),
        }
    }
}

enum ServiceKind {
    Node(usize),
    Database,
}

impl<'a> Display for ExtraLabels<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let operator = self.operator.as_str();

        match self.service_kind {
            ServiceKind::Node(idx) => {
                write!(
                    f,
                    "operator=\"{operator}\",service_kind=\"node\",node_idx=\"{idx}\""
                )
            }
            ServiceKind::Database => {
                write!(f, "operator=\"{operator}\",service_kind=\"db\"")
            }
        }
    }
}

async fn append_extra_labels(metrics: String, extra_labels: ExtraLabels<'_>) -> String {
    let mut buf = String::with_capacity(metrics.len() * 2);

    for (idx, line) in metrics.lines().enumerate() {
        let log_invalid_line = || tracing::warn!("Invalid line: {line}");

        let line = line.trim();

        // Drop both HELP and TYPE.
        //
        // We don't really use HELP anyway, and TYPE is not allowed to be duplicated, so
        // handling it would require more complicated logic.
        //
        // TYPE is not required in Prometheus, and it should work just fine without it.
        if line.is_empty() | line.starts_with('#') {
            continue;
        }

        let Some(first_word_end_idx) = line.find(' ') else {
            continue;
        };

        let first_word = &line[..first_word_end_idx];

        let (insert_idx, has_braces, has_labels) = if let Some(lb_idx) = first_word.find('{') {
            let Some(rb_idx) = line.rfind('}') else {
                log_invalid_line();
                continue;
            };

            if lb_idx > rb_idx {
                log_invalid_line();
                continue;
            }

            let in_braces = &line[lb_idx..=rb_idx];
            let has_labels = in_braces.chars().any(|ch| !ch.is_whitespace());

            (rb_idx, true, has_labels)
        } else {
            (first_word_end_idx, false, false)
        };

        buf.push_str(&line[..insert_idx]);

        if !has_braces {
            buf.push('{');
        }

        if has_labels {
            buf.push(',');
        }

        // `Write` impl of `String` does not error.
        let _ = write!(buf, "{extra_labels}");

        if !has_braces {
            buf.push('}');
        }

        buf.push_str(&line[insert_idx..]);

        buf.push('\n');

        // Yield every 100 lines
        if idx % 100 == 0 {
            tokio::task::yield_now().await;
        }
    }

    buf
}

#[cfg(test)]
mod test {
    use super::*;

    const METRICS: &str = "\
# TYPE wcn_available_memory gauge
wcn_available_memory 8194981888

# TYPE wcn_rpc_server_available_rpc_permits gauge
wcn_rpc_server_available_rpc_permits{server_name=\"primary\"} 20000

# TYPE future_cancelled_duration summary
future_cancelled_duration{future_name=\"wcn_rpc_server_inbound_connection\",quantile=\"0\"} 0
future_cancelled_duration{future_name=\"wcn_rpc_server_inbound_connection\",quantile=\"0.5\"} 0";

    #[rustfmt::skip]
    const PROCESSED_NODE_METRICS: &str = "\
wcn_available_memory{operator=\"OperatorA\",service_kind=\"node\",node_idx=\"0\"} 8194981888
wcn_rpc_server_available_rpc_permits{server_name=\"primary\",operator=\"OperatorA\",service_kind=\"node\",node_idx=\"0\"} 20000
future_cancelled_duration{future_name=\"wcn_rpc_server_inbound_connection\",quantile=\"0\",operator=\"OperatorA\",service_kind=\"node\",node_idx=\"0\"} 0
future_cancelled_duration{future_name=\"wcn_rpc_server_inbound_connection\",quantile=\"0.5\",operator=\"OperatorA\",service_kind=\"node\",node_idx=\"0\"} 0
";

    #[rustfmt::skip]
    const PROCESSED_DATABASE_METRICS: &str = "\
wcn_available_memory{operator=\"OperatorA\",service_kind=\"db\"} 8194981888
wcn_rpc_server_available_rpc_permits{server_name=\"primary\",operator=\"OperatorA\",service_kind=\"db\"} 20000
future_cancelled_duration{future_name=\"wcn_rpc_server_inbound_connection\",quantile=\"0\",operator=\"OperatorA\",service_kind=\"db\"} 0
future_cancelled_duration{future_name=\"wcn_rpc_server_inbound_connection\",quantile=\"0.5\",operator=\"OperatorA\",service_kind=\"db\"} 0
";

    #[tokio::test]
    async fn append_node_labels() {
        let operator = node_operator::Name::new("OperatorA").unwrap();

        assert_eq!(
            PROCESSED_NODE_METRICS,
            append_extra_labels(METRICS.to_string(), ExtraLabels::node(&operator, 0)).await
        )
    }

    #[tokio::test]
    async fn append_database_labels() {
        let operator = node_operator::Name::new("OperatorA").unwrap();

        assert_eq!(
            PROCESSED_DATABASE_METRICS,
            append_extra_labels(METRICS.to_string(), ExtraLabels::database(&operator)).await
        )
    }
}
