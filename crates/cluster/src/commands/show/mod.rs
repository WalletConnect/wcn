use crate::commands::operator::update::UpdatedNodeOperator;

#[derive(Debug, clap::Args)]
pub struct ShowCmd {
    #[command(flatten)]
    shared_args: super::SharedArgs,
}

pub async fn exec(cmd: ShowCmd) -> anyhow::Result<()> {
    let cluster = cmd.shared_args.new_client().await?;

    let cluster_view = cluster.view();

    let operators_read = cluster_view
        .node_operators()
        .slots()
        .iter()
        .flatten()
        .cloned()
        .map(|op| op.into())
        .collect::<Vec<UpdatedNodeOperator>>();

    let operators = serde_json::to_string_pretty(&operators_read)?;

    println!("Node operators:\n{}", operators);

    Ok(())
}
