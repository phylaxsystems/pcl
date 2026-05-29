use super::{
    ApiCommandError,
    HttpMethod,
};

pub(crate) fn generated_operation_path(
    operation_id: &str,
    path_params: &[(&str, &str)],
) -> Option<String> {
    let (_, template) = generated_operation_template(operation_id)?;
    expand_path_template(operation_id, template, path_params.iter().copied()).ok()
}

#[cfg(test)]
pub(in crate::api) fn generated_operation_templates()
-> &'static [(&'static str, HttpMethod, &'static str)] {
    generated_operation_templates_impl()
}

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

    pub(in crate::api) fn path_params(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.path_params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
    }

    pub(in crate::api) fn replace_path_param(&mut self, name: &'static str, value: String) {
        if let Some((_, existing_value)) = self
            .path_params
            .iter_mut()
            .find(|(existing_name, _)| *existing_name == name)
        {
            *existing_value = value;
        }
    }

    pub(in crate::api) fn path(&self) -> Result<String, ApiCommandError> {
        let (method, template) =
            generated_operation_template(self.operation_id).ok_or_else(|| {
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

        expand_path_template(
            self.operation_id,
            template,
            self.path_params
                .iter()
                .map(|(name, value)| (*name, value.as_str())),
        )
    }
}

pub(in crate::api) fn generated_operation_template(
    operation_id: &str,
) -> Option<(HttpMethod, &'static str)> {
    dapp_api_client::generated::operation_paths::OPERATION_PATHS
        .iter()
        .find_map(|(candidate_id, method, template)| {
            (*candidate_id == operation_id).then_some((method_from_generated(method)?, *template))
        })
}

#[cfg(test)]
fn generated_operation_templates_impl() -> &'static [(&'static str, HttpMethod, &'static str)] {
    static TEMPLATES: std::sync::OnceLock<Vec<(&'static str, HttpMethod, &'static str)>> =
        std::sync::OnceLock::new();
    TEMPLATES
        .get_or_init(|| {
            dapp_api_client::generated::operation_paths::OPERATION_PATHS
                .iter()
                .filter_map(|(operation_id, method, template)| {
                    Some((*operation_id, method_from_generated(method)?, *template))
                })
                .collect()
        })
        .as_slice()
}

fn method_from_generated(method: &str) -> Option<HttpMethod> {
    match method {
        "GET" => Some(HttpMethod::Get),
        "POST" => Some(HttpMethod::Post),
        "PUT" => Some(HttpMethod::Put),
        "PATCH" => Some(HttpMethod::Patch),
        "DELETE" => Some(HttpMethod::Delete),
        _ => None,
    }
}

fn expand_path_template<'a>(
    operation_id: &str,
    template: &str,
    path_params: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<String, ApiCommandError> {
    let mut path = template.to_string();
    for (name, value) in path_params {
        let encoded = encode_path_segment(value);
        path = path.replace(&format!("{{{name}}}"), &encoded);
    }
    if path.contains('{') || path.contains('}') {
        return Err(ApiCommandError::InvalidWorkflow {
            message: format!(
                "Missing path parameter for generated OpenAPI operation `{operation_id}`"
            ),
        });
    }
    Ok(path)
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
            "get_views_incidents_incident_id_transactions_invalidating_transaction_id_trace",
        )
        .path_param("incidentId", "incident 1")
        .path_param("invalidatingTransactionId", "0xabc/def")
        .path()
        .unwrap();

        assert_eq!(
            path,
            "/views/incidents/incident%201/transactions/0xabc%2Fdef/trace"
        );
    }
}
