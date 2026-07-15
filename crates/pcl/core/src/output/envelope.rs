use crate::output::{
    actions::normalize_next_actions_for_mode,
    human::human_string,
};
use pcl_common::args::OutputMode;
use serde_json::{
    Value,
    json,
};
use std::io::Write as _;

pub const ENVELOPE_SCHEMA_VERSION: &str = "pcl.envelope.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStatus {
    Ok,
    Error,
    Warning,
    Pending,
    ActionRequired,
}

impl EnvelopeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Pending => "pending",
            Self::ActionRequired => "action_required",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
    StdoutOrStderrForFailure,
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("Failed to encode output: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Failed to write output: {0}")]
    Io(#[from] std::io::Error),
}

#[allow(clippy::needless_pass_by_value)]
pub fn ok_envelope(data: Value, next_actions: impl Into<Vec<String>>) -> Value {
    with_envelope_metadata(json!({
        "status": EnvelopeStatus::Ok.as_str(),
        "data": data,
        "next_actions": next_actions.into(),
    }))
}

pub fn error_envelope(
    code: &str,
    message: &str,
    recoverable: bool,
    next_actions: impl Into<Vec<String>>,
) -> Value {
    with_envelope_metadata(json!({
        "status": EnvelopeStatus::Error.as_str(),
        "error": {
            "code": code,
            "message": message,
            "recoverable": recoverable,
        },
        "next_actions": next_actions.into(),
    }))
}

pub fn with_envelope_metadata(mut value: Value) -> Value {
    if let Value::Object(object) = &mut value {
        object
            .entry("schema_version")
            .or_insert_with(|| json!(ENVELOPE_SCHEMA_VERSION));
        object
            .entry("pcl_version")
            .or_insert_with(|| json!(env!("CARGO_PKG_VERSION")));
    }
    value
}

pub fn print_envelope(
    value: &Value,
    output_mode: OutputMode,
    stream: OutputStream,
) -> Result<(), OutputError> {
    let output = envelope_output_string(value, output_mode)?;
    let is_error = value.get("status").and_then(Value::as_str) == Some("error");
    match (stream, is_error) {
        (OutputStream::Stdout, _) | (OutputStream::StdoutOrStderrForFailure, false) => {
            print!("{output}");
            std::io::stdout().flush()?;
        }
        (OutputStream::Stderr, _) | (OutputStream::StdoutOrStderrForFailure, true) => {
            eprint!("{output}");
            std::io::stderr().flush()?;
        }
    }
    Ok(())
}

pub fn envelope_output_string(
    value: &Value,
    output_mode: OutputMode,
) -> Result<String, OutputError> {
    let mut value = with_envelope_metadata(value.clone());
    normalize_next_actions_for_mode(&mut value, output_mode);
    match output_mode {
        OutputMode::Json => Ok(format!("{}\n", serde_json::to_string_pretty(&value)?)),
        OutputMode::Human => Ok(human_string(&value)),
    }
}
