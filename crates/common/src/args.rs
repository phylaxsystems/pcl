use clap::Parser;

#[derive(Debug, Parser, Clone, Default)]
pub struct CliArgs {
    #[clap(short, long)]
    json: bool,
}

impl CliArgs {
    pub fn json_output(&self) -> bool {
        self.json
    }
}
