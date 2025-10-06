use {
    crate::ClusterArgs,
    anyhow::Context,
    clap::{Args, Subcommand},
    std::str::FromStr,
};

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Add a new Node Operator
    Add(AddOperatorArgs),

    /// Remove an existing Node Operator
    Remove(RemoveOperatorArgs),
}

#[derive(Debug, Args)]
pub(super) struct AddOperatorArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// ID of the Node Operator
    #[arg(long = "id", short)]
    id: wcn_cluster::node_operator::Id,

    /// Name of the Node Operator
    #[arg(long = "name", short)]
    name: wcn_cluster::node_operator::Name,

    /// Initial set of Nodes
    #[arg(long = "node")]
    nodes: Vec<Node>,
}

#[derive(Debug, Args)]
pub(super) struct RemoveOperatorArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// ID of the Node Operator to remove
    #[arg(long = "id", short)]
    id: wcn_cluster::node_operator::Id,
}

pub(super) async fn execute(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Add(args) => add_operator(args).await,
        Command::Remove(args) => remove_operator(args).await,
    }
}

async fn add_operator(args: AddOperatorArgs) -> anyhow::Result<()> {
    let node_operator = wcn_cluster::NodeOperator::new(
        args.id,
        args.name,
        args.nodes.into_iter().map(|node| node.0).collect(),
        vec![],
    )
    .context("NodeOperator::new")?;

    args.cluster_args
        .connect()
        .await?
        .add_node_operator(node_operator)
        .await
        .context("Cluster::add_node_operator")
}

async fn remove_operator(args: RemoveOperatorArgs) -> anyhow::Result<()> {
    args.cluster_args
        .connect()
        .await?
        .remove_node_operator(args.id)
        .await
        .context("Clutser::remove_node_operator")
}

#[derive(Clone, Debug)]
struct Node(wcn_cluster::Node);

impl FromStr for Node {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split(' ');
        let peer_id = parts.next().context("peer_id")?.parse()?;
        let ipv4_addr = parts.next().context("ipv4_addr")?.parse()?;

        let private_ipv4_addr = parts.next().context("private_ipv4_addr")?;
        let private_ipv4_addr = if private_ipv4_addr == "None" {
            None
        } else {
            Some(private_ipv4_addr.parse()?)
        };

        let primary_port = parts.next().context("primary_port")?.parse()?;
        let secondary_port = parts.next().context("secondary_port")?.parse()?;

        if parts.next().is_some() {
            return Err(anyhow::anyhow!("Too many parts"));
        }

        Ok(Self(wcn_cluster::Node {
            peer_id,
            ipv4_addr,
            private_ipv4_addr,
            primary_port,
            secondary_port,
        }))
    }
}
