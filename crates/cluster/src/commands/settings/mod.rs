#![allow(warnings)]

use wcn_cluster::EncryptionKey;

mod update;

#[derive(Debug, clap::Args)]
pub struct SettingsCmd {
    #[command(flatten)]
    shared_args: super::SharedArgs,

    #[command(subcommand)]
    commands: SettingsSub,
}

#[derive(clap::Subcommand, Debug)]
pub enum SettingsSub {
    Update(update::UpdateCmd),
}

pub async fn exec(cmd: SettingsCmd) -> anyhow::Result<()> {
    let client = cmd.shared_args.new_client().await?;

    let encryption_key = EncryptionKey::from_hex(&cmd.shared_args.encryption_key)
        .map_err(|e| anyhow::anyhow!("Failed to decode encryption key: {}", e))?;

    match cmd.commands {
        SettingsSub::Update(cmd) => update::exec(cmd, client, encryption_key).await,
        _ => {
            todo!()
        }
    }
}
