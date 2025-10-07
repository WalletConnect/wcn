use crate::ClusterArgs;

pub(super) async fn execute(args: ClusterArgs) -> anyhow::Result<()> {
    let cluster = args.connect().await?;
    let cluster_view = cluster.view();

    for (idx, slot) in cluster_view.node_operators().slots().iter().enumerate() {
        let Some(operator) = slot else {
            println!("{idx}: None");
            continue;
        };

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

        println!();
    }

    let mut keyspace_operators: Vec<_> = cluster_view.keyspace().operators().collect();
    keyspace_operators.sort();
    println!("keyspace: {keyspace_operators:?}");

    if let Some(migration) = cluster_view.migration() {
        let new_keyspace = migration.keyspace().unwrap();
        let mut new_keyspace_operators: Vec<_> = new_keyspace.operators().collect();
        new_keyspace_operators.sort();
        let mut pulling_operators: Vec<_> = migration.pulling_operators().unwrap().iter().collect();
        pulling_operators.sort();
        println!("new_keyspace: {new_keyspace_operators:?}");
        println!("pulling_operators: {pulling_operators:?}");
    }

    if let Some(maintenance) = cluster_view.maintenance() {
        println!("maintenance: {}", maintenance.slot());
    }

    Ok(())
}
