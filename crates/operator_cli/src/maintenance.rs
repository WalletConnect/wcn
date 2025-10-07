use {crate::ClusterArgs, anyhow::Context, clap::Subcommand};

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start maintenance
    Start(ClusterArgs),

    /// Finish maintenance
    Finish(ClusterArgs),
}

pub(super) async fn execute(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Start(args) => start_maintenance(args).await,
        Command::Finish(args) => finish_maintenance(args).await,
    }
}

async fn start_maintenance(args: ClusterArgs) -> anyhow::Result<()> {
    args.connect()
        .await?
        .start_maintenance()
        .await
        .context("Cluster::start_maintenance")
}

async fn finish_maintenance(args: ClusterArgs) -> anyhow::Result<()> {
    args.connect()
        .await?
        .finish_maintenance()
        .await
        .context("Cluster::finish_maintenance")
}
