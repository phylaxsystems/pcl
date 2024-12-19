use clap::{command, Parser};
use eyre::Result;
use pcl_phoundry::{build::BuildArgs, Phoundry, PhoundryError};
use pcl_common::args::CliArgs;

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
    #[command(flatten)]
    args: CliArgs,
}

#[derive(clap::Subcommand)]
enum Commands {
    Phoundry(Phoundry),
    Build(BuildArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Check if forge is installed
    Phoundry::forge_must_be_installed()?;

    let cli = Cli::parse();
    match cli.command {
        Commands::Phoundry(phoundry) => {
            let _ = phoundry.run(cli.args.clone(), true)?;
            Ok::<(), PhoundryError>(())
        }
        Commands::Build(build) => {
            build.run(cli.args.clone())?;
            Ok::<(), PhoundryError>(())
        }
    }?;
    Ok(())
}