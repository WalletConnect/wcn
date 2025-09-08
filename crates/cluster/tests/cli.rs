use alloy::{
    node_bindings::{Anvil, AnvilInstance},
    signers::local::PrivateKeySigner,
};
use wcn_cluster::{
    node_operator,
    smart_contract::{self, Signer},
    Client, Cluster, Node, NodeOperator, Settings, SmartContract,
};

fn new_signer(n: u8, anvil: &AnvilInstance) -> Signer {
    let private_key_signer: PrivateKeySigner = anvil.keys()[n as usize].clone().into();
    let private_key = &format!("{:#x}", private_key_signer.to_bytes());
    Signer::try_from_private_key(private_key).unwrap()
}

fn operator_id(n: u8, anvil: &AnvilInstance) -> node_operator::Id {
    *new_signer(n, anvil).address()
}

fn new_node_operator(n: u8, anvil: &AnvilInstance) -> NodeOperator {
    testing::node_operator(0).tap_mut(|op| op.id = operator_id(n, anvil))
}

async fn provider(signer: Signer, anvil: &AnvilInstance) -> RpcProvider {
    let ws_url = anvil.endpoint_url().to_string().replace("http://", "ws://");
    RpcProvider::new(ws_url.parse().unwrap(), signer)
        .await
        .unwrap()
}

#[derive(AsRef, Clone, Copy)]
struct Config {
    #[as_ref]
    encryption_key: EncryptionKey,
}

impl wcn_cluster::Config for Config {
    type SmartContract = evm::SmartContract;
    type KeyspaceShards = ();
    type Node = Node;

    fn new_node(&self, _operator_id: node_operator::Id, node: Node) -> Self::Node {
        node
    }
}

#[tokio::test]
pub async fn cli_test_suite() {
    // Spin up a local Anvil instance automatically
    let anvil = Anvil::new()
        .block_time(1)
        .chain_id(31337)
        .try_spawn()
        .unwrap();

    let cfg = Config {
        encryption_key: wcn_cluster::testing::encryption_key(),
    };

    let settings = Settings {
        max_node_operator_data_bytes: 4096,
    };

    // Use Anvil's first key for deployment - convert PrivateKeySigner to our Signer
    let private_key_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let signer =
        Signer::try_from_private_key(&format!("{:#x}", private_key_signer.to_bytes())).unwrap();

    let provider = provider(signer, &anvil).await;

    let operators: Vec<_> = (1..=5).map(|n| new_node_operator(n, &anvil)).collect();

    let cluster = Cluster::deploy(cfg, &provider, settings, operators.clone())
        .await
        .unwrap();

    let sc = cluster.smart_contract();

    let key_bytes = anvil.keys()[0].to_bytes().to_vec();
    let key = hex::encode(key_bytes);

    test_keygen().unwrap();
    test_deploy(&anvil, &key, operators.clone(), cfg).unwrap();
    test_update(&anvil, &key, operators, cfg, sc.address().unwrap()).unwrap();
    // test_migration_start(&anvil, key, sc).unwrap();
}

#[allow(dead_code)]
fn test_migration_start(
    anvil: &AnvilInstance,
    key: &str,
    sc: &impl SmartContract,
) -> anyhow::Result<()> {
    let mut cmd = assert_cmd::Command::cargo_bin("wcn_cluster").unwrap();
    let sc_addr = sc.address().unwrap().to_string();

    cmd.arg("migration")
        .arg("-k")
        .arg(key)
        .arg("--provider-url")
        .arg(anvil.ws_endpoint_url().to_string())
        .arg("--contract-address")
        .arg(sc_addr)
        .arg("start")
        .assert()
        .success();

    Ok(())
}

fn test_deploy(
    anvil: &AnvilInstance,
    key: &str,
    initial_operators: Vec<NodeOperator>,
    cfg: Config,
) -> anyhow::Result<()> {
    let mut cmd = assert_cmd::Command::cargo_bin("wcn_cluster").unwrap();
    let operators = serde_json::to_string_pretty(&initial_operators).unwrap();

    let encryption_key = cfg.encryption_key.to_hex();

    cmd.arg("deploy")
        .arg("-s")
        .arg(key)
        .arg("--provider-url")
        .arg(anvil.ws_endpoint_url().to_string())
        .arg("--encryption-key")
        .arg(encryption_key)
        .arg("--operators")
        .arg(operators)
        .assert()
        .success();

    Ok(())
}

fn test_update(
    anvil: &AnvilInstance,
    key: &str,
    operators: Vec<NodeOperator>,
    cfg: Config,
    address: smart_contract::Address,
) -> anyhow::Result<()> {
    let mut cmd = assert_cmd::Command::cargo_bin("wcn_cluster").unwrap();

    let mut op = operators.get(2).unwrap().clone();
    op.name = node_operator::Name::new("UpdatedName").unwrap();

    // NOTE: in real usage, it shouldn't be a node's peer id
    let peer_id = op.nodes().first().unwrap().peer_id;

    let new_client = Client {
        peer_id,
        authorized_namespaces: vec![100, 101].into(),
    };

    op.clients = vec![new_client];

    let operators = vec![op];
    let operators = serde_json::to_string(&operators).unwrap();

    let encryption_key = cfg.encryption_key.to_hex();

    let out = cmd
        .arg("operator")
        .arg("--signer-key")
        .arg(key)
        .arg("--provider-url")
        .arg(anvil.ws_endpoint_url().to_string())
        .arg("--encryption-key")
        .arg(encryption_key)
        .arg("--contract-address")
        .arg(address.to_string())
        .arg("update")
        .arg("--operators")
        .arg(operators)
        .arg("--view-after-update")
        .assert()
        .success()
        .get_output()
        .clone();

    let expected = r#"Updated node operators:
[
  {
    "id": "0x90f79bf6eb2c4f870365e785982e1f101e93b906",
    "name": "UpdatedName",
    "clients": [
      {
        "peer_id": "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN",
        "authorized_namespaces": [
          100,
          101
        ]
      }
    ],
    "nodes": [
      {
        "peer_id": "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN",
        "ipv4_addr": "10.0.0.0",
        "primary_port": 3000,
        "secondary_port": 3001
      },
      {
        "peer_id": "12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp",
        "ipv4_addr": "10.0.0.1",
        "primary_port": 3000,
        "secondary_port": 3001
      }
    ],
    "counter": 18446744073709551615
  }
]
Updated 1 operator(s)
"#;

    assert_eq!(String::from_utf8_lossy(&out.stdout), String::from(expected));

    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct TestKeygenOutput {
    secret_key: String,
    public_key: String,
    peer_id: String,
}

fn test_keygen() -> anyhow::Result<()> {
    let mut cmd = assert_cmd::Command::cargo_bin("wcn_cluster").unwrap();

    let k1_str = cmd
        .arg("key")
        .arg("generate")
        .assert()
        .success()
        .get_output()
        .clone();

    let k1: TestKeygenOutput = serde_json::from_slice(&k1_str.stdout).unwrap();

    let mut cmd = assert_cmd::Command::cargo_bin("wcn_cluster").unwrap();
    let k2_str = cmd
        .arg("key")
        .arg("generate")
        .arg("--secret-key")
        .arg(k1.secret_key.clone())
        .assert()
        .success()
        .get_output()
        .clone();

    let k2: TestKeygenOutput = serde_json::from_slice(&k2_str.stdout).unwrap();

    assert_eq!(k1.public_key, k2.public_key);
    assert_eq!(k1.peer_id, k2.peer_id);

    Ok(())
}
