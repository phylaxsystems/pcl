macro_rules! action {
    (
        $name:literal, $auth:literal, $operation_id:literal, $example:literal
        $(, required: [$($required:literal),* $(,)?])?
        $(, optional: [$($optional:literal),* $(,)?])?
        $(, body_template: $body_template:literal)?
        $(, required_body: [$($required_body:literal),* $(,)?])?
        $(, query: {$($query_key:literal => $query_value:literal),* $(,)?})?
        $(,)?
    ) => {
        super::super::definitions::WorkflowActionDefinition {
            name: $name,
            auth: $auth,
            operation_id: $operation_id,
            required_flags: &[$($($required),*)?],
            optional_flags: &[$($($optional),*)?],
            required_body_fields: &[$($($required_body),*)?],
            body_template: optional_literal!($($body_template)?),
            query: &[$($(($query_key, $query_value)),*)?],
            example: $example,
        }
    };
}

macro_rules! workflow_definition {
    (
        $name:literal,
        command: $command:literal,
        description: $description:literal,
        output: $output:literal,
        policy: $policy:ident,
        actions: [$($action:expr),* $(,)?],
    ) => {
        pub(in crate::api) const DEFINITION: super::super::definitions::WorkflowDefinition =
            super::super::definitions::WorkflowDefinition {
                name: $name,
                command: $command,
                description: $description,
                output: $output,
                output_policy: super::super::definitions::WorkflowOutputPolicy::$policy,
                actions: &[$($action),*],
            };
    };
}

macro_rules! optional_literal {
    () => {
        None
    };
    ($value:literal) => {
        Some($value)
    };
}

pub(in crate::api) mod access;
pub(in crate::api) mod account;
pub(in crate::api) mod assertions;
pub(in crate::api) mod contracts;
pub(in crate::api) mod deployments;
pub(in crate::api) mod events;
pub(in crate::api) mod incidents;
pub(in crate::api) mod integrations;
pub(in crate::api) mod projects;
pub(in crate::api) mod protocol_manager;
pub(in crate::api) mod releases;
pub(in crate::api) mod search;

pub(super) use access::access_request;
pub(super) use account::account_request;
pub(super) use assertions::{
    assertions_next_actions,
    assertions_request,
};
pub(super) use contracts::{
    contracts_next_actions,
    contracts_request,
};
pub(super) use deployments::{
    compact_deployment_data,
    deployments_request,
};
pub(super) use events::events_request;
pub(super) use incidents::{
    incidents_next_actions,
    incidents_request,
};
pub(super) use integrations::integrations_request;
pub(super) use projects::{
    projects_next_actions,
    projects_request,
};
pub(super) use protocol_manager::{
    protocol_manager_next_actions,
    protocol_manager_request,
};
pub(super) use releases::{
    releases_next_actions,
    releases_request,
};
pub(super) use search::{
    search_next_actions,
    search_request,
};

use super::{
    ApiCommandError,
    ProjectsArgs,
    WorkflowOperation,
    WorkflowRequest,
    read_body,
};
use serde_json::{
    Map,
    Value,
    json,
};
use std::path::PathBuf;

fn workflow_operation_with_body(
    operation: WorkflowOperation,
    require_auth: bool,
    body: Option<String>,
    next_actions: impl IntoIterator<Item = impl Into<String>>,
) -> Result<WorkflowRequest, ApiCommandError> {
    WorkflowRequest::from_operation(operation, Vec::new(), body, require_auth, next_actions)
}

fn workflow_operation_get(
    operation: WorkflowOperation,
    require_auth: bool,
    next_actions: impl IntoIterator<Item = impl Into<String>>,
) -> Result<WorkflowRequest, ApiCommandError> {
    workflow_operation_get_with_query(operation, Vec::new(), require_auth, next_actions)
}

fn workflow_operation_get_with_query(
    operation: WorkflowOperation,
    query: Vec<(String, String)>,
    require_auth: bool,
    next_actions: impl IntoIterator<Item = impl Into<String>>,
) -> Result<WorkflowRequest, ApiCommandError> {
    WorkflowRequest::from_operation(operation, query, None, require_auth, next_actions)
}

fn body_or_empty(body: Option<String>) -> String {
    body.unwrap_or_else(|| "{}".to_string())
}

pub(super) fn request_body(
    body: Option<&str>,
    body_file: Option<&PathBuf>,
    fields: &[String],
) -> Result<Option<String>, ApiCommandError> {
    let body = read_body(body, body_file)?;
    body_with_fields(body, fields)
}

fn project_request_body(args: &ProjectsArgs) -> Result<Option<String>, ApiCommandError> {
    let body = read_body(args.body.as_deref(), args.body_file.as_ref())?;
    let mut object = match body {
        Some(body) => serde_json::from_str::<Value>(&body)?,
        None => Value::Object(Map::new()),
    };
    let Value::Object(map) = &mut object else {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "project body must be a JSON object".to_string(),
        });
    };

    insert_optional(
        map,
        "project_name",
        args.project_name.clone().map(Value::String),
    );
    insert_optional(
        map,
        "project_description",
        args.project_description.clone().map(Value::String),
    );
    insert_optional(
        map,
        "profile_image_url",
        args.profile_image_url.clone().map(Value::String),
    );
    insert_optional(
        map,
        "github_url",
        args.github_url.clone().map(Value::String),
    );
    insert_optional(map, "chain_id", args.chain_id.map(|value| json!(value)));
    insert_optional(map, "is_private", args.is_private.map(|value| json!(value)));
    insert_optional(map, "is_dev", args.is_dev.map(|value| json!(value)));
    apply_fields(map, &args.field)?;

    if map.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::Object(map.clone()).to_string()))
    }
}

fn body_with_fields(
    body: Option<String>,
    fields: &[String],
) -> Result<Option<String>, ApiCommandError> {
    if fields.is_empty() {
        return Ok(body);
    }
    let mut value = match body {
        Some(body) => serde_json::from_str::<Value>(&body)?,
        None => Value::Object(Map::new()),
    };
    let Value::Object(map) = &mut value else {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--field requires the request body to be a JSON object".to_string(),
        });
    };
    apply_fields(map, fields)?;
    Ok(Some(Value::Object(map.clone()).to_string()))
}

fn apply_fields(map: &mut Map<String, Value>, fields: &[String]) -> Result<(), ApiCommandError> {
    for field in fields {
        let (key, value) = field.split_once('=').ok_or_else(|| {
            ApiCommandError::InvalidKeyValue {
                kind: "field",
                input: field.clone(),
            }
        })?;
        map.insert(key.to_string(), parse_field_value(value));
    }
    Ok(())
}

fn parse_field_value(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}

fn insert_optional(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_string(), value);
    }
}

fn required_arg(value: Option<&str>, name: &str) -> Result<String, ApiCommandError> {
    value.map(ToString::to_string).ok_or_else(|| {
        ApiCommandError::InvalidWorkflow {
            message: format!("{name} is required"),
        }
    })
}

fn required_arg_with_actions(
    value: Option<&str>,
    name: &str,
    next_actions: Vec<String>,
) -> Result<String, ApiCommandError> {
    value.map(ToString::to_string).ok_or_else(|| {
        ApiCommandError::InvalidWorkflowWithActions {
            message: format!("{name} is required"),
            next_actions,
        }
    })
}

fn required_project_arg(
    value: Option<&str>,
    command: &str,
    flag: &str,
) -> Result<String, ApiCommandError> {
    required_arg_with_actions(
        value,
        flag,
        vec![
            "pcl projects mine".to_string(),
            format!("pcl {command} {flag} <project-id>"),
            format!("pcl {command} --help"),
        ],
    )
}

pub(super) fn first_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(value) = object.get(*key).and_then(Value::as_str) {
                    return Some(value.to_string());
                }
            }
            object
                .values()
                .find_map(|value| first_string_field(value, keys))
        }
        Value::Array(values) => {
            values
                .iter()
                .find_map(|value| first_string_field(value, keys))
        }
        _ => None,
    }
}

fn redact_large_artifacts(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut redacted = Map::new();
            for (key, value) in object {
                let value = if is_large_artifact_key(key) {
                    artifact_redaction(value)
                } else {
                    redact_large_artifacts(value)
                };
                redacted.insert(key.clone(), value);
            }
            Value::Object(redacted)
        }
        Value::Array(values) => Value::Array(values.iter().map(redact_large_artifacts).collect()),
        _ => value.clone(),
    }
}

fn is_large_artifact_key(key: &str) -> bool {
    let normalized = key.replace(['_', '-'], "").to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "sourcecode"
            | "soliditysource"
            | "bytecode"
            | "deploymentbytecode"
            | "runtimebytecode"
            | "creationbytecode"
    )
}

fn artifact_redaction(value: &Value) -> Value {
    match value {
        Value::String(source) => {
            json!({
                "redacted": true,
                "bytes": source.len(),
                "reason": "large_artifact"
            })
        }
        Value::Array(values) => {
            json!({
                "redacted": true,
                "items": values.len(),
                "reason": "large_artifact"
            })
        }
        Value::Object(object) => {
            json!({
                "redacted": true,
                "fields": object.len(),
                "reason": "large_artifact"
            })
        }
        _ => Value::Null,
    }
}

fn push_query<T: ToString>(query: &mut Vec<(String, String)>, name: &str, value: Option<T>) {
    if let Some(value) = value {
        query.push((name.to_string(), value.to_string()));
    }
}
