use clap::Parser;
use foundry_cli::forge::Forge;

const VERSION_MESSAGE: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("VERGEN_GIT_SHA"),
    " ",
    env!("VERGEN_BUILD_TIMESTAMP"),
    ")"
);

#[derive(Parser)]
#[command(
    name = "cl",
    version = VERSION_MESSAGE,
    about = "Command line interface for Phylax Systems tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Forge commands for smart contract development
    Forge(forge::Forge),
    
    /// Local development and testing tools
    Dev {
        #[arg(short, long)]
        verbose: bool,
    },
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Forge(cmd) => cmd.run().await?,
        Commands::Dev { verbose } => {
            if verbose {
                println!("Running in verbose mode");
            }
        }
    }

    Ok(())
}
