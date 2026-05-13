use serde_json::Value;

/// Render a JSON value as the CLI's compact TOON-style text output.
pub fn toon_string(value: &Value) -> String {
    let mut output = toon_format::encode_default(value).unwrap_or_else(|_| {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    });
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}
