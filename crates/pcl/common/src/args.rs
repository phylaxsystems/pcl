use clap::{
    Parser,
    ValueEnum,
};
use std::{
    fmt,
    path::PathBuf,
    sync::atomic::{
        AtomicU8,
        Ordering,
    },
};

static CURRENT_OUTPUT_MODE: AtomicU8 = AtomicU8::new(OutputMode::Human as u8);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputMode {
    #[default]
    Human,
    Toon,
    Json,
}

pub fn set_current_output_mode(mode: OutputMode) {
    CURRENT_OUTPUT_MODE.store(mode as u8, Ordering::Relaxed);
}

pub fn current_output_mode() -> OutputMode {
    match CURRENT_OUTPUT_MODE.load(Ordering::Relaxed) {
        value if value == OutputMode::Toon as u8 => OutputMode::Toon,
        value if value == OutputMode::Json as u8 => OutputMode::Json,
        _ => OutputMode::Human,
    }
}

impl fmt::Display for OutputMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Human => "human",
            Self::Toon => "toon",
            Self::Json => "json",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MachineOutputMode {
    Toon,
    Json,
}

impl fmt::Display for MachineOutputMode {
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
        conflicts_with = "toon",
        help = "Emit strict JSON output for programmatic consumers"
    )]
    pub json: bool,
    #[clap(
        long,
        global = true,
        conflicts_with = "json",
        help = "Emit compact TOON output for agents"
    )]
    pub toon: bool,
    #[clap(
        long = "format",
        global = true,
        value_enum,
        hide = true,
        help = "Deprecated alias for --toon or --json"
    )]
    pub format: Option<MachineOutputMode>,
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
    pub fn output_mode(&self) -> OutputMode {
        if self.json || self.format == Some(MachineOutputMode::Json) {
            OutputMode::Json
        } else if self.toon || self.format == Some(MachineOutputMode::Toon) {
            OutputMode::Toon
        } else {
            OutputMode::Human
        }
    }

    pub fn json_output(&self) -> bool {
        self.output_mode() == OutputMode::Json
    }

    pub fn toon_output(&self) -> bool {
        self.output_mode() == OutputMode::Toon
    }

    pub fn human_output(&self) -> bool {
        self.output_mode() == OutputMode::Human
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
        assert_eq!(args.output_mode(), OutputMode::Json);
    }

    #[test]
    fn parses_toon_flag() {
        let args = CliArgs::try_parse_from(["cli", "--toon"]).expect("should parse");
        assert!(args.toon_output());
        assert_eq!(args.output_mode(), OutputMode::Toon);
    }

    #[test]
    fn parses_legacy_format_json_flag() {
        let args = CliArgs::try_parse_from(["cli", "--format", "json"]).expect("should parse");
        assert!(args.json_output());
        assert_eq!(args.format, Some(MachineOutputMode::Json));
        assert_eq!(args.output_mode(), OutputMode::Json);
    }

    #[test]
    fn parses_legacy_format_toon_flag() {
        let args = CliArgs::try_parse_from(["cli", "--format", "toon"]).expect("should parse");
        assert!(args.toon_output());
        assert_eq!(args.format, Some(MachineOutputMode::Toon));
        assert_eq!(args.output_mode(), OutputMode::Toon);
    }

    #[test]
    fn defaults_to_human_output() {
        let args = CliArgs::try_parse_from(["cli"]).expect("should parse");
        assert!(args.human_output());
        assert!(!args.json_output());
        assert!(!args.toon_output());
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
