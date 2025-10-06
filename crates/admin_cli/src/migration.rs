use {
    crate::ClusterArgs,
    anyhow::Context,
    clap::{Args, Subcommand},
    std::str::FromStr,
    wcn_cluster::node_operator,
};

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Starts a new migration
    Start(StartMigrationArgs),
}

#[derive(Debug, Args)]
pub(super) struct StartMigrationArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// ID or index of the Node Operator to add to the keyspace
    #[arg(long, short)]
    add: Vec<OperatorIdentifier>,

    /// ID or index of the Node Operator to remove from the keyspace
    #[arg(long, short)]
    remove: Vec<OperatorIdentifier>,
}

pub(super) async fn execute(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Start(args) => start_migration(args).await,
    }
}

async fn start_migration(args: StartMigrationArgs) -> anyhow::Result<()> {
    let cluster = args.cluster_args.connect().await?;
    let cluster_view = cluster.view();
    let operators = cluster_view.node_operators();

    let find_operator = |ident| match ident {
        OperatorIdentifier::Id(id) => operators
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("NodeOperator(id: {id}) not found")),
        OperatorIdentifier::Idx(idx) => operators
            .get_by_idx(idx)
            .ok_or_else(|| anyhow::anyhow!("NodeOperator(idx: {idx}) not found")),
    };

    let add: Vec<_> = args
        .add
        .into_iter()
        .map(find_operator)
        .collect::<Result<_, _>>()?;

    let remove: Vec<_> = args
        .remove
        .into_iter()
        .map(find_operator)
        .collect::<Result<_, _>>()?;

    if !add.is_empty() {
        println!("\nadding:");
        for operator in &add {
            println!("\t{} ({})", operator.name, operator.id);
        }
        println!();
    }

    if !remove.is_empty() {
        println!("\nremoving:");
        for operator in &remove {
            println!("\t{} ({})", operator.name, operator.id);
        }
        println!();
    }

    let plan = wcn_cluster::migration::Plan {
        remove: remove.iter().map(|op| op.id).collect(),
        add: add.iter().map(|op| op.id).collect(),
        replication_strategy: wcn_cluster::keyspace::ReplicationStrategy::UniformDistribution,
    };

    if crate::ask_approval()? {
        cluster
            .start_migration(plan)
            .await
            .context("Cluster::start_migration")?;
    }

    Ok(())
}

#[derive(Clone, Debug)]
enum OperatorIdentifier {
    Id(node_operator::Id),
    Idx(u8),
}

impl FromStr for OperatorIdentifier {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(idx) = u8::from_str(s) {
            return Ok(Self::Idx(idx));
        }

        Ok(Self::Id(node_operator::Id::from_str(s)?))
    }
}
