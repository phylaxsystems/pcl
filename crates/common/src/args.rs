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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_default_args() {
        let args = CliArgs::default();
        assert_eq!(args.assertions_dir(), PathBuf::from("assertions"));
        assert_eq!(args.assertions_src(), PathBuf::from("assertions/src"));
        assert_eq!(args.assertions_test(), PathBuf::from("assertions/test"));
    }

    #[test]
    fn test_custom_dir() {
        let args = CliArgs {
            assertions_dir: Some(PathBuf::from("/custom/path")),
        };
        assert_eq!(args.assertions_dir(), PathBuf::from("/custom/path"));
        assert_eq!(args.assertions_src(), PathBuf::from("/custom/path/src"));
        assert_eq!(args.assertions_test(), PathBuf::from("/custom/path/test"));
    }

    #[test]
    fn test_env_var() {
        env::set_var("PCL_ROOT", "/env/path");
        let args = CliArgs::try_parse_from(["program"]).unwrap();
        assert_eq!(args.assertions_dir(), PathBuf::from("/env/path"));
        env::remove_var("PCL_ROOT");
    }

    #[test]
    fn test_cli_override() {
        env::set_var("PCL_ROOT", "/env/path");
        let args = CliArgs::try_parse_from(["program", "-d", "/cli/path"]).unwrap();
        assert_eq!(args.assertions_dir(), PathBuf::from("/cli/path"));
        env::remove_var("PCL_ROOT");
    }
}
