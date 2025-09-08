use {
    crate::commands::{
        parse_operators_from_str, read_operators_from_file, ClusterConfig, SharedArgs,
    },
    derive_more::AsRef,
    itertools::Itertools,
    serde::Serialize,
    std::path::PathBuf,
    wcn_cluster::{
        node_operator::{Id, Name},
        Client, Cluster, EncryptionKey, Node, NodeOperator, NodeOperators, SmartContract,
    },
};

#[derive(Debug, clap::Args)]
pub struct UpdateCmd {
    #[clap(long = "operators-file", short = 'n')]
    /// Path to a file containing a serialized list node operators to update.
    operators_file: Option<PathBuf>,

    #[clap(long = "operators", short = 'o')]
    /// JSON string containing a serialized list of node operators to update.
    operators: Option<String>,

    #[clap(long = "view-after-update", short = 'r')]
    /// Whether to issue a call to read the current list of node operators after
    /// updating.
    view_after_update: bool,

    #[clap(long = "dry-run", short = 'd', default_value_t = false)]
    dry: bool,

    #[clap(long = "verbose", short = 'v', default_value_t = false)]
    verbose: bool,
}

#[derive(AsRef, Clone, Debug, Serialize)]
pub struct UpdatedNodeOperator<N = Node> {
    /// ID of this [`NodeOperator`].
    #[as_ref]
    pub id: Id,

    /// Name of the [`NodeOperator`].
    pub name: Name,

    /// List of [`Client`]s authorized to use the WCN cluster on behalf of the
    /// [`NodeOperator`].
    pub clients: Vec<Client>,

    /// List of [`Node`]s of the [`NodeOperator`].
    pub nodes: Vec<N>,
}

impl From<NodeOperator> for UpdatedNodeOperator {
    fn from(op: NodeOperator) -> Self {
        let nodes = op.nodes().into_iter().cloned().collect();

        let NodeOperator {
            id, name, clients, ..
        } = op;

        Self {
            id,
            name,
            clients,
            nodes,
        }
    }
}

// TODO: refactor to cleanup and improve error handling and reporting
pub async fn exec<S: SmartContract>(
    cmd: UpdateCmd,
    client: S,
    encryption_key: EncryptionKey,
) -> anyhow::Result<()> {
    let operators = {
        if let Some(operators) = cmd.operators {
            parse_operators_from_str(&operators)?
        } else if let Some(path) = cmd.operators_file {
            read_operators_from_file(&path).await?
        } else {
            vec![]
        }
    };

    let target_ids = operators.iter().map(|op| op.id).collect::<Vec<_>>();

    let mut successful_updates = 0;

    let cfg = ClusterConfig { encryption_key };

    // TODO: dont stop on first error
    // TODO: refactor to fold
    for operator in operators {
        if cmd.verbose {
            println!("Updating node operator: {}", &operator.id);
        }

        if cmd.dry {
            println!("Dry run, not updating operator {}", &operator.id);
            continue;
        }

        if cmd.verbose {
            println!("serializing operator: {}", &operator.id);
        }
        let op = operator.serialize(&cfg.encryption_key)?;

        client.update_node_operator(op).await?;

        successful_updates += 1;
    }

    if cmd.view_after_update {
        let cluster_view = client.cluster_view().await?;

        let operators_read: Vec<UpdatedNodeOperator> = cluster_view
            .node_operators
            .into_iter()
            .flatten()
            .map(|sop| sop.deserialize(&cfg))
            .filter_ok(|op| target_ids.contains(&op.id))
            .map_ok(|op| op.into())
            .try_collect()?;

        let operators = serde_json::to_string_pretty(&operators_read).unwrap();

        println!("Updated node operators:\n{}", operators);
    }

    println!("Updated {} operator(s)", successful_updates);

    Ok(())
}
