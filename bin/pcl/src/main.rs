use clap::{command, Parser};
use eyre::Result;
use pcl_phoundry::Phoundry;

const VERSION_MESSAGE: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\nCommit: ",
    env!("VERGEN_GIT_SHA"),
    "\nBuild Timestamp: ",
    env!("VERGEN_BUILD_TIMESTAMP"),
);

#[derive(Parser)]
#[command(
    name = "pcl",
    version = VERSION_MESSAGE,
    about = "The Credible CLI for the Credible Layer"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Phoundry(Phoundry),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Phoundry(phoundry) => phoundry.run(phoundry.args.clone()),
    }?;
    Ok(())
}

