## Operator CLI

The `wcn_operator` application allows node operators to manage their smart-contract state. The application can be run from the repository via:

```bash
$ cargo run --bin wcn_operator -- --help
WCN Node Operator CLI

Usage: wcn_operator <COMMAND>

Commands:
  view         Get overview of your Node Operator
  key          Key management
  node         Node management
  client       Client management
  maintenance  Maintenance scheduling
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

> **NOTE**: All `wcn_operator` commands print help messages when `--help` is passed as argument.

### View

Provides a view of your nodes:

```bash
$ cargo run --bin wcn_operator -- view
```

### Client Management

#### Add or update a Client

```bash
$ cargo run --bin wcn_operator -- client set <options>
```

#### Remove a Client

```bash
$ cargo run --bin wcn_operator -- client remove <options>
```

### Nodes Management

#### Add a Node

```bash
$ cargo run --bin wcn_operator -- node add <options>
```

#### Update a Node

```bash
$ cargo run --bin wcn_operator -- node update <options>
```

#### Remove a Node

```bash
$ cargo run --bin wcn_operator -- node remove <options>
```

### Maintenance

Maintenance mode allows you to signal the cluster that your setup could be offline while under maintenance. This is useful for e.g. updating the database.

> **NOTE**: only one Node Operator is allowed to be in maintenance mode at a given time.

#### Start maintenance

Signals to the cluster that you have entered maintenance mode:

```bash
$ cargo run --bin wcn_operator -- maintenance start --private-key <PRIVATE_KEY> --encryption-key <ENCRYPTION_KEY> --contract-address <CONTRACT_ADDRESS> --rpc-provider-url <RPC_PROVIDER_URL>
```

#### Finish maintenance

Signals to the cluster that you have exited maintenance mode:

```bash
$ cargo run --bin wcn_operator -- maintenance finish --private-key <PRIVATE_KEY> --encryption-key <ENCRYPTION_KEY> --contract-address <CONTRACT_ADDRESS> --rpc-provider-url <RPC_PROVIDER_URL>
```
