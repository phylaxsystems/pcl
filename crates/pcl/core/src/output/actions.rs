use pcl_common::args::OutputMode;
use serde_json::Value;

pub enum NextAction {
    Command(String),
    Text(String),
}

impl NextAction {
    pub fn command(command: impl Into<String>) -> Self {
        Self::Command(command.into())
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn into_string_for_mode(self, mode: OutputMode) -> String {
        match self {
            Self::Command(command) => command_for_mode(&command, mode),
            Self::Text(text) => text,
        }
    }
}

pub fn command_for_mode(command: &str, mode: OutputMode) -> String {
    let command = strip_mode_flags(command);
    match mode {
        OutputMode::Human => command,
        OutputMode::Toon => format!("{command} --toon"),
        OutputMode::Json => format!("{command} --json"),
    }
}

pub fn normalize_next_actions_for_mode(value: &mut Value, mode: OutputMode) {
    let Some(actions) = value.get_mut("next_actions").and_then(Value::as_array_mut) else {
        return;
    };

    for action in actions {
        let Some(command) = action
            .as_str()
            .filter(|action| action.trim_start().starts_with("pcl "))
        else {
            continue;
        };
        *action = Value::String(command_for_mode(command, mode));
    }
}

fn strip_mode_flags(command: &str) -> String {
    command
        .replace(" --format toon", "")
        .replace(" --format json", "")
        .replace(" --toon", "")
        .replace(" --json", "")
        .replace("--toon ", "")
        .replace("--json ", "")
}
