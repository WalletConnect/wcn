## Architecture Overview

The WalletConnect Network 2.0. architecture is organized as follows:

- A group of regional clusters of nodes containing each a minimum of 5 node operators
- Node operators which run a minium of 2 wcn-node instances and at least 1 wcn-db instance in their respective region
- A Smart Contract per region where state is decentralized and shared among node operator software deployments.

## Operator Onboarding

### Generate cryptographic key-pairs for your deployments

Pull the repository and run the following

```
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

You will need to store the private keys you generate in a secure place and avoid losing them as well as the `peer_id`  for each key as that is used by our team to identify and allow your deployments to join the network.

**Note:** Peer IDs are derived from public keys, thus they may be freely shared without posing a security risk.

## Prepare your infrastructure

As pointed out before, operators are expected to run at least one wcn-db instance and at least two wcn-node instances, with the latter configured to point to this aforementioned wcn-db instance.

### Database configuration

This is the database component mentioned before and as with any database, it requires disk space to store information in it. It is highly advisable this information is not deleted or tampered with in any way and if there’s plans to wipe the information therein, please notify our team to receive guidance on how to go through the deployment decommissioning process.

In terms of security and availability, you should keep this gated behind a firewall and only let your wcn-node deployments interact with it, however it’s highly advisable you keep the metrics port open so our monitoring systems can collect metrics.

**Note:** this component is not optional and implements the core functionality of the network, data storage, thus it important operators see to its availability and data integrity.

Operators can use the wcn-db container image we provide here: https://github.com/WalletConnect/wcn/pkgs/container/wcn-db

```
SECRET_KEY: <your secret key>
PRIMARY_RPC_SERVER_PORT: 30010
SECONDARY_RPC_SERVER_PORT: 30011
METRICS_SERVER_PORT: 30012
ROCKSDB_DIR: "/wcn/rocksdb"
```

### Node configuration

This component acts as a stateless request coordinator within the network and it’s important that it can access a the regional Cluster Smart Contract via some RPC provider. The address for the contract on each region will be provided by the team.

It’s also very important that any firewall that gates this component allows inbound and outbound UDP and TCP traffic to and from all ports specified below.

**Note:** failing to deploy or correctly configure this component negatively impacts the network by reducing availability and affects incentives for network participants.

Operators can use the wcn-node container image we provide here: https://github.com/WalletConnect/wcn/pkgs/container/wcn-node

```
SECRET_KEY: <your secret key>
SMART_CONTRACT_ADDRESS: <the address of the cluster smart contract for your region>
RPC_PROVIDER_URL: <an RPC provider URL, e.g. from Infura or Alchemy or your own node>
DATABASE_PEER_ID: <the peer ID of your database node>
DATABASE_RPC_SERVER_ADDRESS: <the address where your database is hosted at>
DATABASE_PRIMARY_RPC_SERVER_PORT: 30010
DATABASE_SECONDARY_RPC_SERVER_PORT: 30011
METRICS_SERVER_PORT: 3017
ORGANIZATION: <your organiztaion's name>
REGION: eu|us|ap|sa
PRIMARY_RPC_SERVER_PORT: 3013
SECONDARY_RPC_SERVER_PORT: 3014
```

### Firewall

Please ensure your firewall configuration allows the ports specified in the Node configuration section of these docs and the metrics port for the wcn-db deployments you host.

## Availability

You MUST always have at least one `wcn_node` online, meaning if there’s an update you should perform a rolling deployment, updating nodes one by one, or at least in batches, so there’s always some nodes still available to serve inbound RPCs. While re-deploying, first verify that the node is operational before continuing with the next one (TODO: specify how to verify → Grafana / Prometheus).

`wcn_db` MUST always be online, if your Database is offline you as node operator (organization) are considered to be fully down and will be penalized accordingly.

In case if you need to restart your Database (re-configuration / update) a “permit” needs to be acquired from the Smart Contract by announcing that you’re about to start a *Maintenance*. Only one Node Operator is allowed to be under *Maintenance* at any given time.

TODO: Docs on how to use CLI to enter maintenance mode
