use {
    anyhow::Context,
    clap::{Args, Parser, Subcommand},
    derive_more::AsRef,
    std::io::{self, Write as _},
    wcn_cluster::{NodeOperator, smart_contract::Write as _},
};

mod key;
mod maintenance;
mod node;
mod view;

type Cluster = wcn_cluster::Cluster<ClusterConfig>;

/// WCN Node Operator CLI.
#[derive(Debug, Parser)]
#[command(name = "wcn_operator", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Get overview of your Node Operator
    View(ClusterArgs),

    /// Key management
    #[command(subcommand)]
    Key(key::Command),

    /// Node management
    #[command(subcommand)]
    Node(node::Command),

    /// Maintenance scheduling
    #[command(subcommand)]
    Maintenance(maintenance::Command),
}

#[derive(Debug, Args)]
struct ClusterArgs {
    /// Private key of your Node Operator (Optimism account keypair)
    #[arg(
        id = "PRIVATE_KEY",
        long = "private-key",
        env = "WCN_NODE_OPERATOR_PRIVATE_KEY"
    )]
    signer: wcn_cluster::smart_contract::evm::Signer,

    /// WCN Cluster Smart-Contract encryption key
    #[arg(
        long = "encryption-key",
        env = "WCN_CLUSTER_SMART_CONTRACT_ENCRYPTION_KEY"
    )]
    encryption_key: wcn_cluster::EncryptionKey,

    /// WCN Cluster Smart-Contract address
    #[arg(long = "contract-address", env = "WCN_CLUSTER_SMART_CONTRACT_ADDRESS")]
    contract_address: wcn_cluster::smart_contract::Address,

    /// Optimism RPC provider URL
    ///
    /// Only ws:// and wss:// are supported
    #[arg(long = "rpc-provider-url", env = "OPTIMISM_RPC_PROVIDER_URL")]
    rpc_provider_url: wcn_cluster::smart_contract::evm::RpcUrl,
}

impl ClusterArgs {
    async fn connect(self) -> anyhow::Result<Cluster> {
        let cfg = ClusterConfig {
            encryption_key: self.encryption_key,
        };

        let connector =
            wcn_cluster::smart_contract::evm::RpcProvider::new(self.rpc_provider_url, self.signer)
                .await
                .context("RpcProvider::new")?;

        Cluster::connect(cfg, &connector, self.contract_address)
            .await
            .context("Cluster::connect")
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::View(args) => view::execute(args).await?,
        Command::Key(cmd) => key::execute(cmd),
        Command::Node(cmd) => node::execute(cmd).await?,
        Command::Maintenance(cmd) => maintenance::execute(cmd).await?,
    }

    Ok(())
}

#[derive(AsRef, Clone, Copy)]
pub(crate) struct ClusterConfig {
    #[as_ref]
    encryption_key: wcn_cluster::EncryptionKey,
}

impl wcn_cluster::Config for ClusterConfig {
    type SmartContract = wcn_cluster::smart_contract::evm::SmartContract;
    type KeyspaceShards = ();
    type Node = wcn_cluster::Node;

    fn new_node(
        &self,
        _operator_id: wcn_cluster::node_operator::Id,
        node: wcn_cluster::Node,
    ) -> Self::Node {
        node
    }
}

fn current_operator(cluster: &Cluster) -> anyhow::Result<NodeOperator<wcn_cluster::Node>> {
    let cluster_view = cluster.view();
    let operator_id = cluster.smart_contract().signer().unwrap();
    cluster_view
        .node_operators()
        .get(operator_id)
        .cloned()
        .context(
            "Provided PRIVATE_KEY does not belong to any of the Node Operators in this WCN Cluster",
        )
}

fn print_node(node: &wcn_cluster::Node) {
    println!("\tPeer ID: {}", node.peer_id);
    println!("\tIPv4 address: {}", node.ipv4_addr);
    if let Some(ip) = node.private_ipv4_addr {
        println!("\tPrivate IPv4 address: {}", ip);
    }
    println!("\tPrimary port: {}", node.primary_port);
    println!("\tSecondary port: {}", node.secondary_port);
    println!();
}

fn ask_approval() -> anyhow::Result<bool> {
    print!("Proceed? (y/yes):");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(["y", "yes"].contains(&input.trim()))
}
