use std::time::Duration;

use clap::{command, Parser};
use eyre::Result;
use pcl_common::args::CliArgs;
use pcl_da::submit::DASubmitArgs;
use pcl_phoundry::{build::BuildArgs, Phorge, PhoundryError};
use indicatif;
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
    Phorge(Phorge),
    Build(BuildArgs),
    DASubmit(DASubmitArgs),
    SubmitToDapp(SubmitToDappArgs),
    Auth(AuthArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Check if forge is installed
    Phorge::forge_must_be_installed()?;

    let cli = Cli::parse();
    let spinner_style = indicatif::ProgressStyle::default_spinner()
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
        .template("{spinner} {msg}")?;

    match cli.command {
        Commands::Phorge(phorge) => {
            let result = phorge.run(cli.args.clone(), true);
            result?;
            Ok::<(), PhoundryError>(())
        }
        Commands::Build(build) => {
            let spinner = indicatif::ProgressBar::new_spinner();
            spinner.set_style(spinner_style.clone());
            spinner.set_message("Compiling Assertions...");
            spinner.enable_steady_tick(Duration::from_secs(4));
            std::thread::sleep(Duration::from_secs(4));
            spinner.finish_with_message("Assertinons compiled!");
            Ok::<(), PhoundryError>(())
        }
        Commands::DASubmit(submit) => {
            let spinner = indicatif::ProgressBar::new_spinner();
            spinner.set_style(spinner_style);
            spinner.set_message(format!("Submitting Assertion {} to the Assertion DA..", submit.assertion));
            spinner.enable_steady_tick(Duration::from_secs(1));
            std::thread::sleep(Duration::from_secs(1));
            spinner.set_message("Assertion DA Verifying Source Code");
            spinner.enable_steady_tick(Duration::from_secs(5));
            std::thread::sleep(Duration::from_secs(5));
            spinner.finish_with_message("Assertions stored in the DA!");
            Ok::<(), PhoundryError>(())
        }
        Commands::SubmitToDapp(submit) => {
            let spinner = indicatif::ProgressBar::new_spinner();
            spinner.set_style(spinner_style);
            let assertions = submit.assertions.join("| ");
            spinner.set_message(format!("Submitting assertions to the Dapp: {}", assertions));
            spinner.enable_steady_tick(Duration::from_secs(3));
            std::thread::sleep(Duration::from_secs(3));
            spinner.finish_with_message("Assertions submitted to the Dapp!");
            Ok::<(), PhoundryError>(())
        }
        Commands::Auth(auth) => {
            let spinner = indicatif::ProgressBar::new_spinner();
            spinner.set_style(spinner_style);
            spinner.set_message("Authenticating with the Credible Layer Dapp");
            spinner.enable_steady_tick(Duration::from_secs(1));
            std::thread::sleep(Duration::from_secs(1));
            spinner.set_message("Please copy the one-time code and visit https://dapp.phylax.systems/device: 852B-DA50");
            spinner.enable_steady_tick(Duration::from_secs(5));
            std::thread::sleep(Duration::from_secs(5));
            spinner.finish_with_message("Authentication succesful!");
            Ok::<(), PhoundryError>(())
        }
    }?;
    Ok(())
}

#[derive(Parser)]
struct SubmitToDappArgs {
    #[clap(long, short)]
    pub assertions: Vec<String>,
    #[clap(long, short)]
    pub project_name: String,
}

#[derive(Parser)]
struct AuthArgs {
}
