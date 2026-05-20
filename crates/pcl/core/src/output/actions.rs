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
    if contains_shell_syntax(command) {
        return command.to_string();
    }

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

pub fn shell_word(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    if !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'/' | b'.' | b'_' | b'-' | b':' | b'@' | b'=')
        })
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
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

fn contains_shell_syntax(command: &str) -> bool {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let chars = command.chars().collect::<Vec<_>>();
    let mut index = 0;

    while let Some(&ch) = chars.get(index) {
        match ch {
            '\'' if !in_double_quote => in_single_quote = !in_single_quote,
            '"' if !in_single_quote => in_double_quote = !in_double_quote,
            '\\' if !in_single_quote => {
                index += 1;
            }
            '<' if !in_single_quote && !in_double_quote => {
                if let Some(close_index) = placeholder_close_index(&chars, index) {
                    index = close_index;
                } else {
                    return true;
                }
            }
            '>' | '|' | '&' | ';' if !in_single_quote && !in_double_quote => return true,
            _ => {}
        }
        index += 1;
    }

    false
}

fn placeholder_close_index(chars: &[char], open_index: usize) -> Option<usize> {
    let mut index = open_index + 1;
    let mut has_content = false;

    while let Some(&ch) = chars.get(index) {
        match ch {
            '>' if has_content => return Some(index),
            '>' | '<' | '|' | '&' | ';' | '\'' | '"' => return None,
            ch if ch.is_whitespace() => return None,
            _ => has_content = true,
        }
        index += 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{
        Value,
        json,
    };

    #[test]
    fn command_for_mode_updates_plain_pcl_commands() {
        assert_eq!(
            command_for_mode("pcl doctor", OutputMode::Toon),
            "pcl doctor --toon"
        );
        assert_eq!(
            command_for_mode("pcl doctor --toon", OutputMode::Json),
            "pcl doctor --json"
        );
        assert_eq!(
            command_for_mode("pcl doctor --json", OutputMode::Human),
            "pcl doctor"
        );
    }

    #[test]
    fn command_for_mode_leaves_shell_snippets_unmodified() {
        let install_command =
            "pcl completions bash > ~/.local/share/bash-completion/completions/pcl";

        assert_eq!(
            command_for_mode(install_command, OutputMode::Toon),
            install_command
        );
        assert_eq!(
            command_for_mode(install_command, OutputMode::Json),
            install_command
        );
    }

    #[test]
    fn command_for_mode_ignores_quoted_shell_characters() {
        assert_eq!(
            command_for_mode("pcl api call get '/health?scope=a&b=c'", OutputMode::Toon),
            "pcl api call get '/health?scope=a&b=c' --toon"
        );
    }

    #[test]
    fn command_for_mode_updates_placeholder_commands() {
        assert_eq!(
            command_for_mode(
                "pcl incidents --project <project-ref> --all --limit 50 --output incidents.json",
                OutputMode::Toon
            ),
            "pcl incidents --project <project-ref> --all --limit 50 --output incidents.json --toon"
        );
        assert_eq!(
            command_for_mode(
                "pcl schema get <workflow> --action <action>",
                OutputMode::Json
            ),
            "pcl schema get <workflow> --action <action> --json"
        );
    }

    #[test]
    fn normalize_next_actions_skips_shell_snippets() {
        let mut envelope = json!({
            "next_actions": [
                "pcl doctor",
                "pcl completions bash > ~/.local/share/bash-completion/completions/pcl",
                "Use the shell completion command without machine output flags"
            ]
        });

        normalize_next_actions_for_mode(&mut envelope, OutputMode::Toon);

        assert_eq!(
            envelope["next_actions"],
            Value::Array(vec![
                Value::String("pcl doctor --toon".to_string()),
                Value::String(
                    "pcl completions bash > ~/.local/share/bash-completion/completions/pcl"
                        .to_string()
                ),
                Value::String(
                    "Use the shell completion command without machine output flags".to_string()
                )
            ])
        );
    }
}
