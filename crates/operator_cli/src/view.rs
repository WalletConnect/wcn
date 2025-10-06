use crate::ClusterArgs;

pub(super) async fn execute(args: ClusterArgs) -> anyhow::Result<()> {
    let cluster = args.connect().await?;

    let operator = crate::current_operator(&cluster)?;

    println!("ID: {}", operator.id);
    println!("Name: {}", operator.name);
    println!("Clients:");
    for client in &operator.clients {
        println!("\tPeer ID: {}", client.peer_id);
        println!(
            "\tAuthorized namespaces: {:?}",
            client.authorized_namespaces
        );
        println!();
    }
    println!("Nodes:");
    for (idx, node) in operator.nodes().iter().enumerate() {
        println!("\tIndex: {}", idx);
        crate::print_node(node);
    }

    if let Some(maintenance) = cluster.view().maintenance()
        && maintenance.slot() == &operator.id
    {
        println!("Maintenance: In progress")
    }

    Ok(())
}
