use {
    crate::{ClusterArgs, print_client},
    anyhow::Context,
    clap::{Args, Subcommand},
    wcn_cluster::PeerId,
};

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Add a new Client or update an existing Client
    Set(SetClientArgs),

    /// Remove an existing Client
    Remove(RemoveClientArgs),
}

#[derive(Debug, Args)]
pub(super) struct SetClientArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// Peer ID of the Client
    #[arg(long, short = 'i')]
    peer_id: PeerId,

    /// Namespace the Client is authorized to use
    #[arg(long = "namespace", short)]
    namespaces: Vec<u8>,

    /// Skip interactive approval of the changes before writing to the
    /// Smart-Contract.
    #[arg(long)]
    auto_approve: bool,
}

#[derive(Debug, Args)]
pub(super) struct RemoveClientArgs {
    #[command(flatten)]
    cluster_args: ClusterArgs,

    /// Peer ID of the Client to remove
    #[arg(long, short = 'i')]
    peer_id: PeerId,

    /// Skip interactive approval of the changes before writing to the
    /// Smart-Contract.
    #[arg(long)]
    auto_approve: bool,
}

pub(super) async fn execute(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Set(args) => add_client(args).await,
        Command::Remove(args) => remove_client(args).await,
    }
}

async fn add_client(args: SetClientArgs) -> anyhow::Result<()> {
    let cluster = args.cluster_args.connect().await?;
    let mut operator = crate::current_operator(&cluster)?;

    let client = operator
        .clients
        .iter_mut()
        .find(|client| client.peer_id == args.peer_id);

    if let Some(client) = client {
        println!("Updating existing Client:");
        println!("Old:");
        print_client(client);

        client.authorized_namespaces = args.namespaces.into();
        println!("New:");
        print_client(client);
    } else {
        let client = wcn_cluster::Client {
            peer_id: args.peer_id,
            authorized_namespaces: args.namespaces.into(),
        };

        println!("Adding a new Client:");
        print_client(&client);

        operator.clients.push(client);
    };

    if args.auto_approve || crate::ask_approval()? {
        cluster
            .update_node_operator(operator)
            .await
            .context("Cluster::update_node_operator")?;
    }

    Ok(())
}

async fn remove_client(args: RemoveClientArgs) -> anyhow::Result<()> {
    let cluster = args.cluster_args.connect().await?;
    let mut operator = crate::current_operator(&cluster)?;

    let idx = operator
        .clients
        .iter()
        .position(|client| client.peer_id == args.peer_id)
        .context("You don't have a Client with the specified Peer ID")?;

    let client = operator.clients.remove(idx);

    println!("Removing Client:");
    crate::print_client(&client);

    if args.auto_approve || crate::ask_approval()? {
        cluster
            .update_node_operator(operator)
            .await
            .context("Cluster::update_node_operator")?;
    }

    Ok(())
}
