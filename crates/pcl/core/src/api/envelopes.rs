use super::{
    WorkflowCallResult,
    definitions::{
        WorkflowOutputPolicy,
        workflow_output_policy,
    },
    workflows::compact_deployment_data,
};
use crate::output::with_envelope_metadata;
use pcl_common::args::OutputMode;
use serde_json::{
    Value,
    json,
};

pub(in crate::api) fn ok_envelope(data: Value) -> Value {
    with_envelope_metadata(json!({
        "status": "ok",
        "data": data,
        "next_actions": [
            "pcl api list",
            "pcl api inspect get_views_public_incidents",
            "pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated",
        ],
    }))
}

pub(in crate::api) fn workflow_success_envelope(
    result: WorkflowCallResult,
    next_actions: Vec<String>,
) -> Value {
    with_envelope_metadata(json!({
        "status": "ok",
        "data": result.body,
        "request": result.request,
        "response": result.response,
        "next_actions": next_actions,
    }))
}

pub(in crate::api) fn workflow_success_envelope_with_data(
    result: WorkflowCallResult,
    data: Value,
    next_actions: Vec<String>,
) -> Value {
    with_envelope_metadata(json!({
        "status": "ok",
        "data": data,
        "request": result.request,
        "response": result.response,
        "next_actions": next_actions,
    }))
}

pub(in crate::api) fn workflow_data_for_output_mode(
    workflow: &str,
    data: &Value,
    output_mode: OutputMode,
) -> Value {
    match (workflow_output_policy(workflow), output_mode) {
        (WorkflowOutputPolicy::MachineRawHumanCompactArtifacts, OutputMode::Human) => {
            compact_deployment_data(data)
        }
        _ => data.clone(),
    }
}

pub(in crate::api) fn query_pairs_value(query: &[(String, String)]) -> Value {
    Value::Array(
        query
            .iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect(),
    )
}

pub(in crate::api) fn upsert_query(query: &mut Vec<(String, String)>, name: &str, value: String) {
    if let Some((_, existing)) = query.iter_mut().find(|(key, _)| key == name) {
        *existing = value;
    } else {
        query.push((name.to_string(), value));
    }
}

pub(in crate::api) fn extract_paginated_items(
    value: &Value,
    preferred_field: &str,
) -> Option<Vec<Value>> {
    if let Some(items) = array_at_path(value, preferred_field) {
        return Some(items.to_vec());
    }
    for path in [
        "items",
        "incidents",
        "results",
        "data.items",
        "data.incidents",
        "data.results",
        "data",
    ] {
        if let Some(items) = array_at_path(value, path) {
            return Some(items.to_vec());
        }
    }
    value.as_array().cloned()
}

fn array_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a [Value]> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        current = current.get(segment)?;
    }
    current.as_array().map(Vec::as_slice)
}
