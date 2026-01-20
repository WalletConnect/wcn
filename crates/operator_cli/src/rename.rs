use {crate::ClusterArgs, anyhow::Context, wcn_cluster::node_operator};

#[derive(Debug, clap::Args)]
pub(super) struct Args {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// New name
    #[arg(long, short)]
    name: node_operator::Name,

    /// Skip interactive approval of the changes before writing to the
    /// Smart-Contract.
    #[arg(long)]
    auto_approve: bool,
}

pub(super) async fn execute(args: Args) -> anyhow::Result<()> {
    let cluster = args.cluster_args.connect().await?;
    let mut operator = crate::current_operator(&cluster)?;

    println!("Current name: {}", operator.name);
    println!("New name: {}", args.name);

    operator.name = args.name;

    if args.auto_approve || crate::ask_approval()? {
        cluster
            .update_node_operator(operator)
            .await
            .context("Cluster::update_node_operator")?;
    }

    Ok(())
}
