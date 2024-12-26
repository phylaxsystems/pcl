use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Clone)]
pub struct CliArgs {
    #[arg(short = 'd', long, env = "PCL_ROOT")]
    pub assertions_dir: Option<PathBuf>,
}

impl CliArgs {
    pub fn assertions_dir(&self) -> PathBuf {
        self.assertions_dir.clone().unwrap_or_default()
    }

    pub fn assertions_src(&self) -> PathBuf {
        self.assertions_dir().join("src")
    }

    pub fn assertions_test(&self) -> PathBuf {
        self.assertions_dir().join("test")
    }
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            assertions_dir: Some(PathBuf::from("assertions")),
        }
    }
}
