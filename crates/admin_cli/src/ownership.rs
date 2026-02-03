use {
    crate::ClusterArgs,
    anyhow::Context,
    clap::{Args, Subcommand},
    wcn_cluster::smart_contract::{AccountAddress, Write as _},
};

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Transfer ownership of the WCN Cluster
    Transfer(TransferArgs),

    /// Accept ownership of the WCN Cluster
    Accept(ClusterArgs),
}

#[derive(Debug, Args)]
pub(super) struct TransferArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// Address of the new owner of the WCN Cluster
    #[arg(long)]
    new_owner: AccountAddress,
}

pub(super) async fn execute(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Transfer(args) => transfer_ownership(args).await,
        Command::Accept(args) => accept_ownership(args).await,
    }
}

async fn transfer_ownership(args: TransferArgs) -> anyhow::Result<()> {
    let cluster = args.cluster_args.connect().await?;

    println!("Transferring ownership to {}", args.new_owner);

    if crate::ask_approval()? {
        cluster
            .transfer_ownership(args.new_owner)
            .await
            .context("Cluster::transfer_ownership")?;
    }

    println!(
        "The complete the transfer the new owner is required to call `wcn_admin ownership accept` \
         command"
    );

    Ok(())
}

async fn accept_ownership(args: ClusterArgs) -> anyhow::Result<()> {
    let cluster = args.connect().await?;

    cluster
        .accept_ownership()
        .await
        .context("Cluster::accept_ownership")?;

    println!(
        "OK. {} is the new owner",
        cluster.smart_contract().signer().unwrap()
    );

    Ok(())
}
