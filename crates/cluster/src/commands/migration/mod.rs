#![allow(warnings)]

use crate::commands::SharedArgs;

#[derive(Debug, clap::Args)]
pub struct MigrationCmd {
    #[clap(flatten)]
    shared_args: SharedArgs,

    #[command(subcommand)]
    commands: MigrationSub,
}

#[derive(clap::Subcommand, Debug)]
pub enum MigrationSub {
    Start(start::StartCmd),
    Complete(complete::CompleteCmd),
    Abort(abort::AbortCmd),
}

pub async fn exec(cmd: MigrationCmd) -> anyhow::Result<()> {
    let client = cmd.shared_args.new_client().await?;

    match cmd.commands {
        MigrationSub::Start(cmd) => start::exec(cmd, client).await,
        MigrationSub::Complete(_) => {
            todo!()
        }
        MigrationSub::Abort(_) => {
            todo!()
        }
    }
}

mod start {
    use {
        crate::commands::ClusterConfig,
        wcn_cluster::{
            keyspace::ReplicationStrategy,
            smart_contract::Write,
            Cluster,
            Keyspace,
            SmartContract,
        },
    };

    #[derive(Debug, clap::Args)]
    pub struct StartCmd {
        // TODO: operators list
    }

    pub async fn exec<C: wcn_cluster::Config>(
        cmd: StartCmd,
        client: Cluster<C>,
    ) -> anyhow::Result<()> {
        let operators = todo!();
        let strategy = ReplicationStrategy::default();
        let version = 0; // grab from contract then increment
        let new_keyspace = Keyspace::new(operators, strategy)?;

        // client.start_migration(new_keyspace.into()).await?;

        Ok(())
    }
}

mod complete {
    #[derive(Debug, clap::Args)]
    pub struct CompleteCmd {}
}

mod abort {
    #[derive(Debug, clap::Args)]
    pub struct AbortCmd {}
}
