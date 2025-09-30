use {
    crate::commands::{
        parse_operators_from_str,
        read_operators_from_file,
        ClusterConfig,
        DEFAULT_CLOCK_SKEW,
        DEFAULT_EVENT_PROPAGATION_LATENCY,
        MAX_NODE_OPERATOR_DATA_BYTES,
    },
    std::{path::PathBuf, time::Duration},
    wcn_cluster::{Cluster, EncryptionKey, Settings},
};

#[derive(Debug, clap::Args)]
pub struct DeployCmd {
    #[command(flatten)]
    shared_args: super::SharedArgs,

    #[clap(long = "operators-file", short = 'n')]
    /// Path to a file containing a serialized list initial node operators.
    operators_file: Option<PathBuf>,

    #[clap(long = "operators", short = 'o')]
    /// JSON string containing a serialized list of initial node operators.
    operators: Option<String>,
}

pub async fn exec(cmd: DeployCmd) -> anyhow::Result<()> {
    let provider = cmd.shared_args.provider().await?;

    // TODO: maybe consider impl a default
    let settings = Settings {
        max_node_operator_data_bytes: MAX_NODE_OPERATOR_DATA_BYTES,
        event_propagation_latency: Duration::from_secs(DEFAULT_EVENT_PROPAGATION_LATENCY),
        clock_skew: Duration::from_millis(DEFAULT_CLOCK_SKEW),
    };

    if cmd.operators.is_none() && cmd.operators_file.is_none() {
        // TODO: discuss if worth emitting a warning instead of an error
        return Err(anyhow::anyhow!(
            "No operators provided, use --operators or --operators-file"
        ));
    }

    let initial_operators = {
        if let Some(operators) = cmd.operators {
            parse_operators_from_str(&operators)?
        } else if let Some(path) = cmd.operators_file {
            read_operators_from_file(&path).await?
        } else {
            vec![]
        }
    };

    let encryption_key = EncryptionKey::from_hex(&cmd.shared_args.encryption_key)
        .map_err(|e| anyhow::anyhow!("Failed to decode encryption key: {}", e))?;

    let cfg = ClusterConfig { encryption_key };

    let cluster = Cluster::deploy(cfg, &provider, settings, initial_operators).await?;

    let address = cluster.smart_contract().address();

    println!("Cluster contract deployed at address: {address}");

    Ok(())
}
