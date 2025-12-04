use {
    crate::ClusterConfig,
    anyhow::Context as _,
    std::net::Ipv4Addr,
    wcn_cluster::{Cluster, Node, NodeOperator, PeerId, Settings, node_operator},
};

#[derive(Debug, clap::Args)]
pub(super) struct Args {
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

    /// Optimism RPC provider URL
    ///
    /// Only ws:// and wss:// are supported
    #[arg(long = "rpc-provider-url", env = "OPTIMISM_RPC_PROVIDER_URL")]
    rpc_provider_url: wcn_cluster::smart_contract::evm::RpcUrl,

    /// Initial set of Node Operators. Minimum of 5 must be provided.
    ///
    /// The initial Node Operator state will contain dummy names and node lists.
    /// To change that each operator will need to be updated separately.
    #[arg(long = "operator", short)]
    operators: Vec<node_operator::Id>,
}

pub(super) async fn execute(args: Args) -> anyhow::Result<()> {
    let deployer =
        wcn_cluster::smart_contract::evm::RpcProvider::new(args.rpc_provider_url, args.signer)
            .await
            .context("RpcProvider::new")?;

    let cfg = ClusterConfig {
        encryption_key: args.encryption_key,
    };

    let settings = Settings::default();

    let operators = args.operators.into_iter().enumerate().map(|(idx, id)| {
        let name = format!("Operator{idx}").parse().unwrap();
        let node = Node {
            peer_id: PeerId::random(),
            ipv4_addr: Ipv4Addr::new(10, 0, 0, 0),
            private_ipv4_addr: None,
            primary_port: 3010,
            secondary_port: 3011,
        };

        NodeOperator::new(id, name, vec![node, node], vec![]).unwrap()
    });

    let cluster = Cluster::deploy(cfg, &deployer, settings, operators.collect())
        .await
        .context("Cluster::deploy")?;

    let address = cluster.smart_contract().address();

    println!("Deployed!");
    println!("Smart-contract address: {address}");

    Ok(())
}
