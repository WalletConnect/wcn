use {
    crate::ClusterArgs,
    anyhow::Context,
    clap::{Args, Subcommand},
    std::net::Ipv4Addr,
    wcn_cluster::PeerId,
};

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Add a new Node
    Add(AddNodeArgs),

    /// Update an existing Node
    Update(UpdateNodeArgs),

    /// Remove an existing Node
    Remove(RemoveNodeArgs),
}

#[derive(Debug, Args)]
pub(super) struct AddNodeArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// Peer ID of the Node
    #[arg(long, short = 'i')]
    peer_id: PeerId,

    /// IPv4 address of the Node
    #[arg(long, short = 'a')]
    ip_address: Ipv4Addr,

    /// Primary RPC server port
    #[arg(long, short)]
    primary_port: u16,

    /// Secondary RPC server port
    #[arg(long, short)]
    secondary_port: u16,

    /// Skip interactive approval of the changes before writing to the
    /// Smart-Contract.
    #[arg(long)]
    auto_approve: bool,
}

#[derive(Debug, Args)]
pub(super) struct UpdateNodeArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// Index of the Node to update
    #[arg(long, short)]
    index: usize,

    /// New Peer ID
    #[arg(long, short = 'i')]
    peer_id: Option<PeerId>,

    /// New IPv4 address
    #[arg(long, short = 'a')]
    ip_address: Option<Ipv4Addr>,

    /// New Primary RPC server port
    #[arg(long, short)]
    primary_port: Option<u16>,

    /// New Secondary RPC server port
    #[arg(long, short)]
    secondary_port: Option<u16>,

    /// Skip interactive approval of the changes before writing to the
    /// Smart-Contract.
    #[arg(long)]
    auto_approve: bool,
}

#[derive(Debug, Args)]
pub(super) struct RemoveNodeArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// Index of the Node to remove
    #[arg(long, short)]
    index: usize,

    /// Skip interactive approval of the changes before writing to the
    /// Smart-Contract.
    #[arg(long)]
    auto_approve: bool,
}

pub(super) async fn execute(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Add(args) => add_node(args).await,
        Command::Update(args) => update_node(args).await,
        Command::Remove(args) => remove_node(args).await,
    }
}

async fn add_node(args: AddNodeArgs) -> anyhow::Result<()> {
    let cluster = args.cluster_args.connect().await?;
    let mut operator = crate::current_operator(&cluster)?;

    let node = wcn_cluster::Node {
        peer_id: args.peer_id,
        ipv4_addr: args.ip_address,
        private_ipv4_addr: None,
        primary_port: args.primary_port,
        secondary_port: args.secondary_port,
    };

    println!("Adding Node:");
    crate::print_node(&node);

    operator.add_node(node);

    if args.auto_approve || crate::ask_approval()? {
        cluster
            .update_node_operator(operator)
            .await
            .context("Cluster::update_node_operator")?;
    }

    Ok(())
}

async fn update_node(args: UpdateNodeArgs) -> anyhow::Result<()> {
    let cluster = args.cluster_args.connect().await?;
    let mut operator = crate::current_operator(&cluster)?;

    let mut node = operator
        .nodes()
        .get(args.index)
        .copied()
        .context("Wrong index")?;

    println!("Updating Node {}:", args.index);
    println!("Old:");
    crate::print_node(&node);

    if let Some(peer_id) = args.peer_id {
        node.peer_id = peer_id;
    }

    if let Some(addr) = args.ip_address {
        node.ipv4_addr = addr;
    }

    if let Some(port) = args.primary_port {
        node.primary_port = port;
    }

    if let Some(port) = args.secondary_port {
        node.secondary_port = port;
    }

    println!("New:");
    crate::print_node(&node);

    operator.update_node(args.index, node);

    if args.auto_approve || crate::ask_approval()? {
        cluster
            .update_node_operator(operator)
            .await
            .context("Cluster::update_node_operator")?;
    }

    Ok(())
}

async fn remove_node(args: RemoveNodeArgs) -> anyhow::Result<()> {
    let cluster = args.cluster_args.connect().await?;
    let mut operator = crate::current_operator(&cluster)?;

    let node = operator
        .nodes()
        .get(args.index)
        .copied()
        .context("Wrong index")?;

    println!("Romoving Node {}:", args.index);
    crate::print_node(&node);

    operator.remove_node(args.index)?;

    if args.auto_approve || crate::ask_approval()? {
        cluster
            .update_node_operator(operator)
            .await
            .context("Cluster::update_node_operator")?;
    }

    Ok(())
}
