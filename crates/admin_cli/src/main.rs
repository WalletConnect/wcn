use {
    anyhow::Context,
    clap::{ArgGroup, Args, Parser, Subcommand},
    derive_more::AsRef,
    std::io::{self, Write as _},
    wcn_cluster::{NodeOperator, smart_contract::evm},
};

mod deploy;
mod migration;
mod operator;
mod ownership;
mod settings;
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

    /// Cluster settings management
    #[command(subcommand)]
    Settings(settings::Command),

    /// Cluster ownership management
    #[command(subcommand)]
    Ownership(ownership::Command),
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("signer")
        .required(true)
        .multiple(false)
        .args(["private_key", "kms_key_id"])
))]
struct ClusterArgs {
    /// Private key of the WCN Cluster Smart-Contract owner
    #[arg(long, env = "WCN_CLUSTER_SMART_CONTRACT_OWNER_PRIVATE_KEY")]
    private_key: Option<String>,

    /// KMS key id of the WCN Cluster Smart-Contract owner
    #[arg(long, env = "WCN_CLUSTER_SMART_CONTRACT_OWNER_KMS_KEY_ID")]
    kms_key_id: Option<String>,

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

        let signer = if let Some(pk) = self.private_key {
            evm::Signer::try_from_private_key(&pk)?
        } else if let Some(key_id) = self.kms_key_id {
            let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            let client = aws_sdk_kms::Client::new(&config);
            evm::Signer::try_from_kms(client, key_id).await?
        } else {
            // `clap` validates that exactly one required argument is present
            unreachable!()
        };

        let connector =
            wcn_cluster::smart_contract::evm::RpcProvider::new(self.rpc_provider_url, signer)
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
        Command::Settings(cmd) => settings::execute(cmd).await,
        Command::Ownership(cmd) => ownership::execute(cmd).await,
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
