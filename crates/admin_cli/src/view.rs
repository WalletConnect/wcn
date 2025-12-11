use crate::ClusterArgs;

pub(super) async fn execute(args: ClusterArgs) -> anyhow::Result<()> {
    let cluster = args.connect().await?;
    let cluster_view = cluster.view();

    println!("{:#?}\n", cluster_view.settings());

    for (idx, slot) in cluster_view.node_operators().slots().iter().enumerate() {
        let Some(operator) = slot else {
            println!("{idx}: None");
            continue;
        };

        crate::print_node_operator(Some(idx as u8), operator);

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
