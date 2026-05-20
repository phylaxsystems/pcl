use super::{
    ApiCommandError,
    HttpMethod,
};

include!(concat!(env!("OUT_DIR"), "/generated_operation_paths.rs"));

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
        let (method, template) = GENERATED_OPERATION_PATHS
            .iter()
            .find_map(|(operation_id, method, template)| {
                (*operation_id == self.operation_id).then_some((*method, *template))
            })
            .ok_or_else(|| {
                ApiCommandError::InvalidWorkflow {
                    message: format!(
                        "Generated OpenAPI operation `{}` was not found",
                        self.operation_id
                    ),
                }
            })?;
        if method != self.method {
            return Err(ApiCommandError::InvalidWorkflow {
                message: format!(
                    "Generated OpenAPI operation `{}` uses method {}, not {}",
                    self.operation_id,
                    method.as_str(),
                    self.method.as_str()
                ),
            });
        }

        let mut path = template.to_string();
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
