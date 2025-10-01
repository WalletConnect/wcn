use {
    crate::commands::{
        DEFAULT_CLOCK_SKEW,
        DEFAULT_EVENT_PROPAGATION_LATENCY,
        MAX_NODE_OPERATOR_DATA_BYTES,
    },
    std::time::Duration,
    wcn_cluster::{Cluster, EncryptionKey, SmartContract},
};

#[derive(Debug, clap::Args)]
pub struct UpdateCmd {
    #[clap(long = "event-propagation-latency", short = 'l', default_value_t = DEFAULT_EVENT_PROPAGATION_LATENCY)]

    /// Maximum expected latency of propagating smart contract events
    /// across the whole cluster.
    event_propagation_latency: u64,

    #[clap(long = "clock-skew", short = 's', default_value_t = DEFAULT_CLOCK_SKEW)]
    clock_skew: u64,

    #[clap(long = "max-data-bytes", short = 'm', default_value_t = MAX_NODE_OPERATOR_DATA_BYTES)]
    /// Maximum number of on-chain bytes stored for a single node operator.
    max_node_operator_data_bytes: u16,

    #[clap(long = "view-after-update", short = 'r')]
    /// Whether to issue a call to read the current list of node operators after
    /// updating.
    view_after_update: bool,

    #[clap(long = "dry-run", short = 'd', default_value_t = false)]
    dry: bool,

    #[clap(long = "verbose", short = 'v', default_value_t = false)]
    verbose: bool,
}

pub async fn exec<C>(
    cmd: UpdateCmd,
    client: Cluster<C>,
    encryption_key: EncryptionKey,
) -> anyhow::Result<()>
where
    C: wcn_cluster::Config,
    C::SmartContract: wcn_cluster::smart_contract::Write,
{
    let UpdateCmd {
        event_propagation_latency,
        clock_skew,
        max_node_operator_data_bytes,
        view_after_update,
        dry,
        verbose,
    } = cmd;

    let new_settings = wcn_cluster::Settings {
        max_node_operator_data_bytes,
        event_propagation_latency: Duration::from_secs(event_propagation_latency),
        clock_skew: Duration::from_millis(clock_skew),
    };

    if dry {
        let updated_settings = serde_json::to_string_pretty(&new_settings)?;
        println!("Dry run, not updating. New settings:\n{}", updated_settings);
        return Ok(());
    }

    client.update_settings(new_settings).await?;

    if cmd.view_after_update {
        let view = client.view();
        let updated_settings = view.settings();
        let updated_settings = serde_json::to_string_pretty(updated_settings)?;

        println!("Updated settings:\n{}", updated_settings);
    }

    Ok(())
}
