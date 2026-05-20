use super::{
    ApiCommandError,
    HttpMethod,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::LazyLock,
};

static OPERATION_PATHS: LazyLock<HashMap<String, (HttpMethod, String)>> = LazyLock::new(|| {
    let spec: Value = serde_json::from_str(include_str!(
        "../../../../dapp-api-client/openapi/spec.json"
    ))
    .expect("cached dapp OpenAPI spec must parse");
    let mut operations = HashMap::new();
    let paths = spec
        .get("paths")
        .and_then(Value::as_object)
        .expect("cached dapp OpenAPI spec must contain paths");

    for (path, path_item) in paths {
        let Some(methods) = path_item.as_object() else {
            continue;
        };
        for (method, operation) in methods {
            let Some(method) = method_from_openapi_key(method) else {
                continue;
            };
            let operation_id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .map_or_else(
                    || generated_operation_id(method.openapi_key(), path),
                    ToString::to_string,
                );
            operations.insert(operation_id, (method, path.clone()));
        }
    }

    operations
});

#[derive(Clone, Debug)]
pub(in crate::api) struct WorkflowOperation {
    pub(in crate::api) method: HttpMethod,
    pub(in crate::api) operation_id: &'static str,
    path_params: Vec<(&'static str, String)>,
}

impl WorkflowOperation {
    pub(in crate::api) fn new(method: HttpMethod, operation_id: &'static str) -> Self {
        Self {
            method,
            operation_id,
            path_params: Vec::new(),
        }
    }

    pub(in crate::api) fn path_param(
        mut self,
        name: &'static str,
        value: impl Into<String>,
    ) -> Self {
        self.path_params.push((name, value.into()));
        self
    }

    pub(in crate::api) fn path(&self) -> Result<String, ApiCommandError> {
        let (method, template) = OPERATION_PATHS.get(self.operation_id).ok_or_else(|| {
            ApiCommandError::InvalidWorkflow {
                message: format!(
                    "Generated OpenAPI operation `{}` was not found",
                    self.operation_id
                ),
            }
        })?;
        if *method != self.method {
            return Err(ApiCommandError::InvalidWorkflow {
                message: format!(
                    "Generated OpenAPI operation `{}` uses method {}, not {}",
                    self.operation_id,
                    method.as_str(),
                    self.method.as_str()
                ),
            });
        }

        let mut path = template.clone();
        for (name, value) in &self.path_params {
            let encoded = encode_path_segment(value);
            path = path.replace(&format!("{{{name}}}"), &encoded);
        }
        if path.contains('{') || path.contains('}') {
            return Err(ApiCommandError::InvalidWorkflow {
                message: format!(
                    "Missing path parameter for generated OpenAPI operation `{}`",
                    self.operation_id
                ),
            });
        }
        Ok(path)
    }
}

fn method_from_openapi_key(method: &str) -> Option<HttpMethod> {
    match method {
        "get" => Some(HttpMethod::Get),
        "post" => Some(HttpMethod::Post),
        "put" => Some(HttpMethod::Put),
        "patch" => Some(HttpMethod::Patch),
        "delete" => Some(HttpMethod::Delete),
        _ => None,
    }
}

fn generated_operation_id(method: &str, path: &str) -> String {
    let path_parts = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(generated_operation_segment)
        .collect::<Vec<_>>()
        .join("_");
    format!("{method}_{path_parts}")
}

fn generated_operation_segment(segment: &str) -> String {
    let segment = segment.trim_matches(|ch| ch == '{' || ch == '}');
    let mut output = String::new();
    let mut previous_was_lower_or_digit = false;

    for ch in segment.chars() {
        if ch.is_ascii_uppercase() {
            if previous_was_lower_or_digit {
                output.push('_');
            }
            output.push(ch.to_ascii_lowercase());
            previous_was_lower_or_digit = false;
        } else if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
            previous_was_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else if !output.ends_with('_') {
            output.push('_');
            previous_was_lower_or_digit = false;
        }
    }

    output.trim_matches('_').to_string()
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());

    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                use std::fmt::Write as _;
                write!(&mut encoded, "%{byte:02X}").expect("writing to a String cannot fail");
            }
        }
    }

    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_operations_expand_and_encode_path_segments() {
        let path = WorkflowOperation::new(
            HttpMethod::Get,
            "get_views_incidents_incident_id_transactions_tx_id_trace",
        )
        .path_param("incidentId", "incident 1")
        .path_param("txId", "0xabc/def")
        .path()
        .unwrap();

        assert_eq!(
            path,
            "/views/incidents/incident%201/transactions/0xabc%2Fdef/trace"
        );
    }
}
