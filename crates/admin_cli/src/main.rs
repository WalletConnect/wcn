use {
    anyhow::Context,
    clap::{Args, Parser, Subcommand},
    derive_more::AsRef,
    std::io::{self, Write as _},
    wcn_cluster::NodeOperator,
};

mod deploy;
mod migration;
mod operator;
mod view;

type Cluster = wcn_cluster::Cluster<ClusterConfig>;

/// WCN Admin CLI.
#[derive(Debug, Parser)]
#[command(name = "wcn_admin", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Deploy a new Cluster
    Deploy(deploy::Args),

    /// Get overview of the Cluster
    View(ClusterArgs),

    /// Operator management
    #[command(subcommand)]
    Operator(operator::Command),

    /// Migration management
    #[command(subcommand)]
    Migration(migration::Command),
}

#[derive(Debug, Args)]
struct ClusterArgs {
    /// Private key of the WCN Cluster Smart-Contract owner
    #[arg(
        id = "PRIVATE_KEY",
        long = "private-key",
        env = "WCN_CLUSTER_SMART_CONTRACT_OWNER_PRIVATE_KEY"
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
        Command::Deploy(args) => deploy::execute(args).await,
        Command::View(args) => view::execute(args).await,
        Command::Operator(cmd) => operator::execute(cmd).await,
        Command::Migration(cmd) => migration::execute(cmd).await,
    }
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

    fn update_settings(&self, _settings: &wcn_cluster::Settings) {}
}

fn ask_approval() -> anyhow::Result<bool> {
    print!("Proceed? (y/yes):");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(["y", "yes"].contains(&input.trim()))
}

fn print_node_operator(idx: Option<u8>, operator: &NodeOperator) {
    let idx = idx.map(|idx| idx.to_string()).unwrap_or("?".to_string());

    println!("operator[{idx}]: {} {}", operator.name, operator.id);

    for (idx, node) in operator.nodes().iter().enumerate() {
        let id = node.peer_id;
        let addr = node.ipv4_addr;
        let priv_addr = node
            .private_ipv4_addr
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "None".to_string());
        let port0 = node.primary_port;
        let port1 = node.secondary_port;

        println!("\tnode[{idx}]: {id} {addr} {priv_addr} {port0} {port1}");
    }

    if !operator.clients.is_empty() {
        for (idx, client) in operator.clients.iter().enumerate() {
            let id = client.peer_id;
            let namespaces = &client.authorized_namespaces;
            println!("\tclient[{idx}]: {id} {namespaces:?}");
        }
    }

    println!()
}
