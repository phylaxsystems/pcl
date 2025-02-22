use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser, Clone, Default)]
pub struct CliArgs {
    /// The root directory for the project.
    /// Defaults to the current directory.
    #[arg(long = "root", env = "PCL_ROOT_DIR")]
    root_dir: Option<PathBuf>,
    /// The directory containing assertions 'src' and 'test' directories.
    /// Defaults to '/assertions' in the root directory.
    #[arg(long = "assertions", env = "PCL_ASSERTIONS_DIR")]
    assertions_dir: Option<PathBuf>,
}

impl CliArgs {
    pub fn root_dir(&self) -> PathBuf {
        self.root_dir.clone().unwrap_or_default()
    }

    pub fn out_dir(&self) -> PathBuf {
        PathBuf::from("out")
    }

    pub fn out_dir_joined(&self) -> PathBuf {
        self.root_dir().join(self.out_dir())
    }

    pub fn assertions_dir(&self) -> PathBuf {
        self.assertions_dir
            .clone()
            .unwrap_or(PathBuf::from("assertions"))
    }

    pub fn assertions_dir_joined(&self) -> PathBuf {
        self.root_dir().join(self.assertions_dir())
    }

    pub fn assertions_src_joined(&self) -> PathBuf {
        self.assertions_dir_joined().join(self.assertions_src())
    }

    pub fn assertions_src(&self) -> PathBuf {
        PathBuf::from("src")
    }

    pub fn assertions_test_joined(&self) -> PathBuf {
        self.assertions_dir_joined().join(self.assertions_test())
    }

    pub fn assertions_test(&self) -> PathBuf {
        PathBuf::from("test")
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
            ..CliArgs::default()
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
