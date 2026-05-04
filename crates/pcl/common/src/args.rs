use clap::{
    Parser,
    ValueEnum,
};
use std::{
    fmt,
    path::PathBuf,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputMode {
    #[default]
    Toon,
    Json,
}

impl fmt::Display for OutputMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Toon => "toon",
            Self::Json => "json",
        })
    }
}

#[derive(Debug, Parser, Clone, Default)]
pub struct CliArgs {
    #[clap(
        short,
        long,
        global = true,
        help = "Alias for --format json; default output is TOON"
    )]
    pub json: bool,
    #[clap(
        long = "format",
        global = true,
        value_enum,
        default_value_t = OutputMode::Toon,
        help = "Select machine-readable envelope format"
    )]
    pub format: OutputMode,
    #[clap(long = "config-dir", hide = true, global = true)]
    pub config_dir: Option<PathBuf>,
    #[clap(
        long = "llms",
        global = true,
        help = "Print a CLI-native LLM usage guide and exit"
    )]
    pub llms: bool,
}

impl CliArgs {
    pub fn json_output(&self) -> bool {
        self.json || self.format == OutputMode::Json
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{
        Path,
        PathBuf,
    };

    #[test]
    fn parses_json_flag() {
        let args = CliArgs::try_parse_from(["cli", "--json"]).expect("should parse");
        assert!(args.json_output());
    }

    #[test]
    fn parses_output_json_flag() {
        let args = CliArgs::try_parse_from(["cli", "--format", "json"]).expect("should parse");
        assert!(args.json_output());
        assert_eq!(args.format, OutputMode::Json);
    }

    #[test]
    fn parses_output_toon_as_default_machine_output() {
        let args = CliArgs::try_parse_from(["cli", "--format", "toon"]).expect("should parse");
        assert!(!args.json_output());
        assert_eq!(args.format, OutputMode::Toon);
    }

    #[test]
    fn parses_llms_flag() {
        let args = CliArgs::try_parse_from(["cli", "--llms"]).expect("should parse");
        assert!(args.llms);
    }

    #[test]
    fn config_dir_can_be_overridden() {
        let parsed = CliArgs::try_parse_from(["cli", "--config-dir", "/tmp/pcl"])
            .expect("should parse hidden config-dir");
        assert_eq!(parsed.config_dir.as_deref(), Some(Path::new("/tmp/pcl")));

        let args = CliArgs {
            config_dir: Some(PathBuf::from("/tmp/pcl")),
            ..Default::default()
        };
        assert_eq!(args.config_dir.as_deref(), Some(Path::new("/tmp/pcl")));
    }
}
