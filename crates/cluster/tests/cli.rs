use {
    alloy::{
        node_bindings::{Anvil, AnvilInstance},
        signers::local::PrivateKeySigner,
    },
    derive_more::AsRef,
    serde::Serialize,
    std::{
        sync::{atomic::AtomicUsize, Arc},
        time::Duration,
    },
    tap::Tap,
    wcn_cluster::{
        node_operator::Id,
        smart_contract::{
            self,
            evm::{RpcProvider, Signer, SmartContract},
        },
        Client,
        Cluster,
        EncryptionKey,
        Node,
        NodeOperator,
        Settings,
    },
};

fn new_signer(n: u8, anvil: &AnvilInstance) -> Signer {
    let private_key_signer: PrivateKeySigner = anvil.keys()[n as usize].clone().into();
    let private_key = &format!("{:#x}", private_key_signer.to_bytes());
    Signer::try_from_private_key(private_key).unwrap()
}

fn operator_id(n: u8, anvil: &AnvilInstance) -> Id {
    *new_signer(n, anvil).address()
}

fn new_node_operator(n: u8, anvil: &AnvilInstance) -> NodeOperator {
    wcn_cluster::testing::node_operator(0).tap_mut(|op| op.id = operator_id(n, anvil))
}

async fn provider(signer: Signer, anvil: &AnvilInstance) -> RpcProvider {
    let ws_url = anvil.endpoint_url().to_string().replace("http://", "ws://");
    RpcProvider::new(ws_url.parse().unwrap(), signer)
        .await
        .unwrap()
}

#[derive(AsRef, Clone, Debug, Serialize)]
pub struct TestNodeOperator<N = Node> {
    #[as_ref]
    pub id: String,
    pub name: wcn_cluster::node_operator::Name,
    pub clients: Vec<Client>,
    nodes: Vec<N>,
    counter: Arc<AtomicUsize>,
}

impl<N> TestNodeOperator<N> {
    pub fn new(
        id: wcn_cluster::node_operator::Id,
        name: wcn_cluster::node_operator::Name,
        clients: Vec<Client>,
        nodes: Vec<N>,
        counter: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            id: id.to_string(),
            name,
            clients,
            nodes,
            counter,
        }
    }
}

impl From<NodeOperator> for TestNodeOperator {
    fn from(op: NodeOperator) -> Self {
        let nodes = op.nodes().to_vec();
        let counter = Default::default();

        let NodeOperator {
            id, name, clients, ..
        } = op;

        Self {
            id: id.to_string(),
            name,
            clients,
            nodes,
            counter,
        }
    }
}

#[derive(AsRef, Clone, Copy)]
struct Config {
    #[as_ref]
    encryption_key: EncryptionKey,
}

impl wcn_cluster::Config for Config {
    type SmartContract = SmartContract;
    type KeyspaceShards = ();
    type Node = Node;

    fn new_node(&self, _operator_id: wcn_cluster::node_operator::Id, node: Node) -> Self::Node {
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
        event_propagation_latency: Duration::from_secs(1),
        clock_skew: Duration::from_millis(100),
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
    test_update(&anvil, &key, operators.clone(), cfg, sc.address()).unwrap();
    test_update_settings(&anvil, &key, cfg, sc.address()).unwrap();
    // test_migration_start(&anvil, key, sc).unwrap();
}

// #[allow(dead_code)]
// fn test_migration_start(
//     anvil: &AnvilInstance,
//     key: &str,
//     sc: &impl SmartContract,
// ) -> anyhow::Result<()> {
//     let mut cmd = assert_cmd::Command::cargo_bin("wcn_cluster").unwrap();
//     let sc_addr = sc.address().unwrap().to_string();
//
//     cmd.arg("migration")
//         .arg("-k")
//         .arg(key)
//         .arg("--provider-url")
//         .arg(anvil.ws_endpoint_url().to_string())
//         .arg("--contract-address")
//         .arg(sc_addr)
//         .arg("start")
//         .assert()
//         .success();
//
//     Ok(())
// }

fn test_deploy(
    anvil: &AnvilInstance,
    key: &str,
    initial_operators: Vec<NodeOperator>,
    cfg: Config,
) -> anyhow::Result<()> {
    let mut cmd = assert_cmd::Command::cargo_bin("wcn_cluster").unwrap();
    let test_operators: Vec<TestNodeOperator> =
        initial_operators.into_iter().map(Into::into).collect();
    let operators = serde_json::to_string_pretty(&test_operators).unwrap();

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
    op.name = wcn_cluster::node_operator::Name::new("UpdatedName").unwrap();

    // NOTE: in real usage, it shouldn't be a node's peer id
    let peer_id = op.nodes().first().unwrap().peer_id;

    let new_client = Client {
        peer_id,
        authorized_namespaces: vec![100, 101].into(),
    };

    op.clients = vec![new_client];

    let operators = vec![op];
    let test_operators: Vec<TestNodeOperator> = operators.into_iter().map(Into::into).collect();
    let operators = serde_json::to_string(&test_operators).unwrap();

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
    ]
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

fn test_update_settings(
    anvil: &AnvilInstance,
    key: &str,
    cfg: Config,
    address: smart_contract::Address,
) -> anyhow::Result<()> {
    let mut cmd = assert_cmd::Command::cargo_bin("wcn_cluster").unwrap();

    let encryption_key = cfg.encryption_key.to_hex();

    let max_data_bytes = 4096;
    let clock_skew = 200;
    let propagation_latency = 50;

    let out = cmd
        .arg("settings")
        .arg("--signer-key")
        .arg(key)
        .arg("--provider-url")
        .arg(anvil.ws_endpoint_url().to_string())
        .arg("--encryption-key")
        .arg(encryption_key)
        .arg("--contract-address")
        .arg(address.to_string())
        .arg("update")
        .arg("--max-data-bytes")
        .arg(max_data_bytes.to_string())
        .arg("--clock-skew")
        .arg(clock_skew.to_string())
        .arg("--event-propagation-latency")
        .arg(propagation_latency.to_string())
        .arg("--view-after-update")
        .assert()
        .success()
        .get_output()
        .clone();

    let expected = r#"Updated settings:
{
  "max_node_operator_data_bytes": 4096,
  "event_propagation_latency": {
    "secs": 50,
    "nanos": 0
  },
  "clock_skew": {
    "secs": 0,
    "nanos": 200000000
  }
}
"#;

    assert_eq!(String::from_utf8_lossy(&out.stdout), String::from(expected));

    Ok(())
}
