use {
    crate::ClusterArgs,
    anyhow::Context,
    clap::{Args, Subcommand},
};

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Update cluster settings
    Update(UpdateSettingsArgs),
}

#[derive(Debug, Args)]
pub(super) struct UpdateSettingsArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// Maximum number of bytes per second transmitted by a single node
    /// during data migration.
    #[arg(long)]
    migration_tx_bandwidth: Option<u32>,

    /// Maximum number of bytes per second received by a single node during
    /// data migration.
    #[arg(long)]
    migration_rx_bandwidth: Option<u32>,
}

pub(super) async fn execute(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Update(args) => update_settings(args).await,
    }
}

async fn update_settings(args: UpdateSettingsArgs) -> anyhow::Result<()> {
    let cluster = args.cluster_args.connect().await?;
    let cluster_view = cluster.view();

    let mut settings = *cluster_view.settings();
    println!("Old: {settings:#?}");

    if let Some(bandwidth) = args.migration_tx_bandwidth {
        settings.migration_tx_bandwidth = bandwidth;
    }

    if let Some(bandwidth) = args.migration_rx_bandwidth {
        settings.migration_rx_bandwidth = bandwidth;
    }

    println!("New: {settings:#?}");

    if crate::ask_approval()? {
        cluster
            .update_settings(settings)
            .await
            .context("Cluster::update_settings")?;
    }

    Ok(())
}
