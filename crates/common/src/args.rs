use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser, Clone, Default)]
pub struct CliArgs {
    /// The root directory for the project.
    /// Defaults to the current directory.
    #[arg(short = 'r', long = "root", env = "PCL_ROOT_DIR")]
    pub root_dir: Option<PathBuf>,
    /// The directory containing assertions 'src' and 'test' directories.
    /// Defaults to '/assertions' in the root directory.
    #[arg(
        short = 's',
        long = "source",
        env = "PCL_ASSERTIONS_DIR",
    )]
    pub assertions_src: Option<PathBuf>,
    #[arg(
        short = 't',
        long = "test",
        env = "PCL_ASSERTIONS_TEST",
    )]
    pub assertions_test: Option<PathBuf>,
    #[arg(
        short = 'p',
        long = "profile",
        env = "PCL_FOUNDRY_PROFILE",
    )]
    pub foundry_profile: Option<String>
}

impl CliArgs {
    pub fn root_dir(&self) -> PathBuf {
        self.root_dir.clone().unwrap_or(PathBuf::from("./"))
    }

    pub fn out_dir(&self) -> PathBuf {
        self.root_dir().join("out")
    }

    pub fn assertions_src(&self) -> PathBuf {
        self.assertions_src
            .clone()
            .unwrap_or(PathBuf::from("assertions/src"))
    }

    pub fn assertions_test(&self) -> PathBuf {
        self.assertions_test
            .clone()
            .unwrap_or(PathBuf::from("assertions/test"))
    }
}