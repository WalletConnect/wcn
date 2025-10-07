use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
#[clap(verbatim_doc_comment)]
#[allow(clippy::large_enum_variant)]
pub(super) enum Command {
    /// Generates ed25519 keypair to be used for WCN Nodes / Database
    /// Not to be confused with Optimism keypair (ECDSA)
    #[clap(verbatim_doc_comment)]
    Generate,

    /// Validates private key previously generated using the `generate` command
    Validate(ValidateArgs),
}

#[derive(Debug, Args)]
pub(super) struct ValidateArgs {
    /// Base64-encoded ed25519 secret key
    #[arg(id = "SECRET_KEY", short = 's', long = "secret-key")]
    keypair: wcn_cluster::PeerKeypair,
}

pub(super) fn execute(cmd: Command) {
    match cmd {
        Command::Generate => print_keypair(&wcn_cluster::PeerKeypair::generate()),
        Command::Validate(args) => print_keypair(&args.keypair),
    }
}

fn print_keypair(kp: &wcn_cluster::PeerKeypair) {
    println!("Secret key: {}", kp.secret_key_base64());
    println!("Peer ID: {}", kp.peer_id());
}
