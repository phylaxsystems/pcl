use clap::{Parser, Subcommand};
use cl_sp1_host::config::PoRUserInputs;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate Proof of Realization
    #[command(name = "por")]
    PoR(PoRUserInputs),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::PoR(inputs) => {
            println!("Generating Proof of Realization with inputs: {:?}", inputs);
        }
    }

    Ok(())
}