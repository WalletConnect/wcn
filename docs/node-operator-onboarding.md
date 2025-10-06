## Architecture Overview

The WalletConnect Network 2.0 architecture is organized as follows:

- A set of regional Clusters, each containing at least 5 Node Operators.
- Node Operators which run at least 2 Nodes (`wcn_node`) and a single Database (`wcn_db`) in their respective region.
- A Smart-Contract (Optimism) per Cluster, in which Cluster management operations are being performed.

## Operator Onboarding

### Inquire your region

First of all, you'll need to contact a WalletConnect team member and negotiate the region your Node Operator will be placed into.

Currently we have Clusters located in Europe, North America, South America and Asia-Pacific.
You may be able to choose your preffered region, or you may be assigned to a specific region, depending on the current state of the network.

After your region is decided you will be provided with the address of the respective Smart-Contract, which you'll need later for the on-boarding process.
Additionally, you will receive a symmetric key we use for encrypting semi-sensitive on-chain data (IP addresses).

### Generate Node Operator keypair

By any means you prefer generate a keypair compatible with the Optimism chain (EVM).
The address of that keypair will be the identifier of your Node Operator in the WalletConnect Network.

You will need to provide the secret key to the env of one of your Nodes, so it's not recommended to reuse an existing keypair.

The account should have some ETH on the Optimism Mainnet. Smart contract calls are inexpensive and infrequent.
Having the equivalent of $5 worth of ETH on Optimism should be sufficient for about a year.

### Generate Node & Database keypairs

Every WCN Node and WCN Database requires a keypair for authentication of network calls.

Your Database should not be exposed outside of your private network, and the necessity of it having a keypair may be removed in the future.
Your Nodes must be exposed to the public internet, and therefore having a keypair for auth is critical.

You can have a separate keypair per Node/Database, or you can use a single keypair for everything.

To generate a keypair pull the repository and run the following:

```bash
 cargo run -p wcn_cluster -F cli key generate
```

It will show you the following (**DO NOT use these example values for your deployments**)

```
{
  "secret_key": "0vr7GpjjVRl3YeUAT4OR6H0K4IcN/diinacBgbZOwJA=",
  "public_key": "eu0nTQ99zlqMHJBQX3z41l7rl3kLR5mgqLDGcyeS0e0=",
  "peer_id": "12D3KpoWJ65FDLgFCrxHDLWkTjq7E9gdW65nwjhWJs97k4WCwxtc"
}
```

You'll need to provide the `secret_key` to the env of your Nodes/Database.

`peer_id` is an alternative representation of `public_key` and it will be mostly used instead of the `public_key`.
You'll need to share `peer_id`s of your Nodes with the WalletConnect team during the on-boarding process.

After being on-boarded you'll be able to update the list of your Nodes (including their `peer_id`s) in the Smart-Contract using our CLI tool.

### Prepare your infrastructure

#### Allocate compute

You'll need to deploy at least 2 Nodes and a single Database.
The current resource requirements:
 - 8CPU / 16GB RAM / 200GB SSD for the Database
 - 4CPU / 8GB RAM for each Node

The requirements may change in the future, in which case there will be a public announcement.

Nodes are stateless and we plan to make them auto-scalable. 

#### Allocate IP address(es)

You'll need to have at least one public static IPv4 address.

You may choose to use the same or different addresses per Node.

### Deploy Database

WCN Database is (of course) stateful. You should never lose the data being stored under any normal conditions.
If there are some exceptional circumstances, please, contact WalletConnect team for guidance.

Your Database should be placed behind a firewall and it should not be accessible via public internet.

You can either build it from source:
```bash
cargo build -p wcn_db --release
```

Or you can use the Docker image we provide: https://github.com/WalletConnect/wcn/pkgs/container/wcn-db

Here's the required environment variables:
```bash
# A `secret_key` previously generated via `key generate` command.
export SECRET_KEY=your_secret_key

# Port of the "primary" RPC server (UDP).
export PRIMARY_RPC_SERVER_PORT=3000

# Port of the "secondary" RPC server (UDP).
export SECONDARY_RPC_SERVER_PORT=3001

# Port of the Prometheus metrics server (TCP).
export METRICS_SERVER_PORT=3002

# Directory in which the persistent data will be stored.
export ROCKSDB_DIR=/wcn/rocksdb
```

You don't need to do regular database backups, we have fault tolerance (via data replication redundancy) on the network level.
However, it makes sense to make a filesystem snapshot if you need to move your database across machines / data centers.

### Choose Optimism RPC provider

WCN Nodes require access to the WCN Cluster smart-contract on the Optimism chain.
In the following section you'll need to specify the URL of the RPC provider to use.

Choose your provider wisely, it should be hosted by an organization you trust.
It would be ideal if you can (or already do) host your own Optimism node.

If you choose to use an external RPC provider prefer a paid one, with good reputation.

If your RPC provider gets compromised or you end up using a malicious one, your Node Operator may experience downtime and your Database data may get corrupted.
Therefore it's your resposibility to ensure RPC provider trustworthiness.

It is also recommended to use RPC providers supporting WebSocket connections (the RPC provider URLs starting with `wss://`).
If you choose to use HTTPs expect your Nodes to issue substantial amount of requests, which may be undesirable if you are paying per request.

### Deploy Nodes

WCN Nodes are stateless.
You must have at least 2 Nodes, or more if you prefer.

Your Nodes may or may not be placed behind a firewall, but if they are make sure that the inbound UDP traffic from the public internet is allowed for the ports you configure. 

You can either build the Node binary from source:
```bash
cargo build -p wcn_node --release
```

Or you can use the Docker image we provide: https://github.com/WalletConnect/wcn/pkgs/container/wcn-node

Here's the required environment variables:
```bash
# Secret key of this Node (`secret_key` you previously generated via `key generate` command).
export SECRET_KEY="<secret key of this node>"

# Port of the "primary" RPC server (UDP).
export PRIMARY_RPC_SERVER_PORT=3010

# Port of the "secondary" RPC server (UDP).
export SECONDARY_RPC_SERVER_PORT=3011

# Port of the Prometheus metrics server (TCP).
export METRICS_SERVER_PORT=3012

# IPv4 address of your WCN Database in your private network.
export DATABASE_RPC_SERVER_ADDRESS=10.0.10.0

# Peer ID of your WCN Database (`peer_id` you previously generated via `key generate` command).
export DATABASE_PEER_ID="<peer id>"

# Port of the "primary" RPC server of your Database. 
export DATABASE_PRIMARY_RPC_SERVER_PORT=3000

# Port of the "secondary" RPC server of your Database. 
export DATABASE_SECONDARY_RPC_SERVER_PORT=3001

# Address of the WCN Cluster smart-contract.
# You should have received it from WalletConnect team after your region has been assigned to you.
export SMART_CONTRACT_ADDRESS="<EVM address (hex)>"

# Symmetric key used for on-chain data encryption.
# You should have received it from WalletConnect team alongside `SMART_CONTRACT_ADDRESS`.
export SMART_CONTRACT_ENCRYPTION_KEY="<symmetric key (hex)>"

# URL of the Optimism RPC provider.
export RPC_PROVIDER_URL="<wss:// or https:// URL>"
```

**IMPORTANT** Exactly _one_ of your Nodes **MUST** also have this configured:
```bash
# Private key of your Node Operator.
# This is from the keypair you generated to be used with Optimism chain.
export SMART_CONTRACT_SIGNER_PRIVATE_KEY="<private key of your Node Operator>"
```
**No matter how many Nodes you have this variable MUST only be configured on one of them!**
Specifying this variable makes the node "primary", which enables data migration management on it.

Your "primary" node is allowed to be offline for short periods of time, for example, during rolling re-deployments.

### Firewall

To re-iterate, please double check the following:
- your Database is behind a firewall and **IS NOT** being exposed to the public internet
- your Nodes **ARE** being exposed to the public internet on `PRIMARY_RPC_SERVER_PORT` (UDP) and `SECONDARY_RPC_SERVER_PORT` (UDP)

### Joining the Cluster

After everything mentioned above is setup, contact WalletConnect team and provide the following:
- your Node Operator ID (Optimism address, the one that corresponds to `SMART_CONTRACT_SIGNER_PRIVATE_KEY`)
- Peer IDs of your Nodes, their IPv4 addresses and `PRIMARY_RPC_SERVER_PORT`/`SECONDARY_RPC_SERVER_PORT` ports

The data you provide will be written to the Smart-Contract.
After this initial write you will be able to update the list of your Nodes yourself using our CLI tool.

## Availability

You **MUST** always have at least one Node online.
Meaning if there’s an update you should perform a rolling deployment, updating nodes one by one, or at least in batches.
This way there should always be at least one Node still available to serve inbound RPCs.

While re-deploying Nodes, first verify that the newly deployed Node is operational before continuing with the next one.
(TODO: Specify how to verify via logs / shared Grafana / Prometheus metrics)

Your Database **MUST** always be online.
If your Database is offline you as a Node Operator (organization) are considered to be fully down and will be penalized accordingly.

In case if you need to restart your Database (re-configuration / update) a "permit" needs to be acquired from the WCN Cluster smart-contract, 
by announcing that you’re about to start a *Maintenance*. Only one Node Operator is allowed to be under *Maintenance* at any given time.
(TODO: Docs on how to use CLI to enter maintenance)
