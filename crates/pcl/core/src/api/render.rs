use super::ApiCommandError;
use crate::output::OutputError;
use pcl_common::args::{
    OutputMode,
    current_output_mode,
};
use serde_json::Value;

pub(super) fn print_output(value: &Value, json_output: bool) -> Result<(), ApiCommandError> {
    print!("{}", envelope_output_string(value, json_output)?);
    Ok(())
}

pub fn envelope_output_string(
    value: &Value,
    json_output: bool,
) -> Result<String, serde_json::Error> {
    let output_mode = if json_output {
        OutputMode::Json
    } else {
        current_output_mode()
    };
    crate::output::envelope_output_string(value, output_mode).map_err(output_error_to_json)
}

fn output_error_to_json(error: OutputError) -> serde_json::Error {
    match error {
        OutputError::Json(error) => error,
        OutputError::Io(error) => serde_json::Error::io(error),
    }
}

pub use crate::output::human_string;
