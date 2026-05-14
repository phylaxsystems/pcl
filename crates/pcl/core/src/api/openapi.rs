use super::{
    ApiCommandError,
    HttpMethod,
    api_manifest,
    method_side_effecting,
};
use serde::Serialize;
use serde_json::{
    Map,
    Value,
    json,
};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{
        Path,
        PathBuf,
    },
};

#[derive(Debug, Serialize)]
pub(super) struct OperationSummary {
    pub(super) operation_id: String,
    pub(super) method: &'static str,
    pub(super) path: String,
    pub(super) summary: Option<String>,
    pub(super) tags: Vec<String>,
    pub(super) auth: Value,
    pub(super) workflow_alternatives: Vec<Value>,
    pub(super) raw_api_use: Value,
    pub(super) inspect_command: String,
    pub(super) call_command: String,
    pub(super) input_placeholders: Vec<String>,
    pub(super) requires_input: bool,
}

#[derive(Clone, Debug)]
struct OperationCoverage {
    operation_id: String,
    method: String,
    path: String,
    hit: u64,
    ok: u64,
    statuses: BTreeMap<String, u64>,
    latest_request_id: Option<String>,
    latest_status: Option<u64>,
    latest_timestamp: Option<String>,
    latest_kind: Option<String>,
}

impl OperationCoverage {
    fn new(operation: &OperationSummary) -> Self {
        Self {
            operation_id: operation.operation_id.clone(),
            method: operation.method.to_string(),
            path: operation.path.clone(),
            hit: 0,
            ok: 0,
            statuses: BTreeMap::new(),
            latest_request_id: None,
            latest_status: None,
            latest_timestamp: None,
            latest_kind: None,
        }
    }

    fn record_hit(&mut self, record: &Value) {
        let status = record.get("status").and_then(Value::as_u64);
        self.hit += 1;
        if status.is_some_and(|status| (200..=299).contains(&status)) {
            self.ok += 1;
        }
        if let Some(status) = status {
            *self.statuses.entry(status.to_string()).or_insert(0) += 1;
        }
        self.latest_status = status;
        self.latest_request_id = record
            .get("request_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.latest_timestamp = record
            .get("timestamp")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.latest_kind = record
            .get("kind")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }

    fn success_2xx(&self) -> bool {
        self.ok > 0
    }

    fn side_effecting(&self) -> bool {
        method_side_effecting(&self.method)
    }

    fn to_value(&self) -> Value {
        json!({
            "operation_id": self.operation_id,
            "method": self.method,
            "path": self.path,
            "hit": self.hit > 0,
            "hits": self.hit,
            "success_2xx": self.success_2xx(),
            "ok": self.ok,
            "statuses": self.statuses,
            "side_effecting": self.side_effecting(),
            "latest_request_id": self.latest_request_id,
            "latest_status": self.latest_status,
            "latest_timestamp": self.latest_timestamp,
            "latest_kind": self.latest_kind,
        })
    }
}

pub(super) fn api_coverage(
    spec: &Value,
    request_log_path: &Path,
    record_limit: usize,
    api_url: &str,
) -> Result<Value, ApiCommandError> {
    let operations = list_operations(spec, None, None)?;
    let records = crate::request_log::read_request_records_at(request_log_path, record_limit)
        .map_err(|source| {
            ApiCommandError::RequestLog {
                path: request_log_path.to_path_buf(),
                source,
            }
        })?;
    let mut coverage = operations
        .iter()
        .map(OperationCoverage::new)
        .collect::<Vec<_>>();
    let mut unmatched_records = Vec::new();

    for record in &records {
        let Some(index) = match_request_record_to_operation(&operations, record) else {
            unmatched_records.push(record.clone());
            continue;
        };
        coverage[index].record_hit(record);
    }

    let mut by_method: BTreeMap<String, BTreeMap<&'static str, u64>> = BTreeMap::new();
    for entry in &coverage {
        let method = by_method.entry(entry.method.clone()).or_default();
        *method.entry("total").or_insert(0) += 1;
        if entry.hit > 0 {
            *method.entry("hit").or_insert(0) += 1;
        }
        if entry.success_2xx() {
            *method.entry("ok").or_insert(0) += 1;
        }
    }

    let no_hit = coverage
        .iter()
        .filter(|entry| entry.hit == 0)
        .map(OperationCoverage::to_value)
        .collect::<Vec<_>>();
    let no_2xx = coverage
        .iter()
        .filter(|entry| entry.hit > 0 && !entry.success_2xx())
        .map(OperationCoverage::to_value)
        .collect::<Vec<_>>();
    let write_no_2xx = coverage
        .iter()
        .filter(|entry| entry.side_effecting() && entry.hit > 0 && !entry.success_2xx())
        .map(OperationCoverage::to_value)
        .collect::<Vec<_>>();
    let operations_value = coverage
        .iter()
        .map(OperationCoverage::to_value)
        .collect::<Vec<_>>();

    Ok(json!({
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "api_url": api_url,
        "request_log": request_log_path,
        "records_considered": records.len(),
        "record_limit": record_limit,
        "total_operations": operations.len(),
        "by_method": by_method,
        "no_hit_count": no_hit.len(),
        "no_2xx_count": no_2xx.len(),
        "write_no_2xx_count": write_no_2xx.len(),
        "unmatched_record_count": unmatched_records.len(),
        "no_hit": no_hit,
        "no_2xx": no_2xx,
        "write_no_2xx": write_no_2xx,
        "unmatched_records": unmatched_records,
        "operations": operations_value,
    }))
}

pub(super) fn write_api_coverage_markdown(
    path: &PathBuf,
    coverage: &Value,
) -> Result<(), ApiCommandError> {
    let markdown = api_coverage_markdown(coverage);
    fs::write(path, markdown).map_err(|source| {
        ApiCommandError::OutputFile {
            path: path.clone(),
            source,
        }
    })
}

pub(super) fn api_coverage_markdown(coverage: &Value) -> String {
    let mut body = String::new();
    writeln!(body, "# PCL API Coverage").expect("writing to String cannot fail");
    writeln!(body).expect("writing to String cannot fail");
    writeln!(
        body,
        "- Generated: {}",
        coverage
            .get("generated_at")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- API URL: {}",
        coverage
            .get("api_url")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- Request log: {}",
        coverage
            .get("request_log")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- Operations: {}",
        coverage
            .get("total_operations")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- Records considered: {}",
        coverage
            .get("records_considered")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- No-hit operations: {}",
        coverage
            .get("no_hit_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- Hit but no 2xx operations: {}",
        coverage
            .get("no_2xx_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )
    .expect("writing to String cannot fail");
    writeln!(body).expect("writing to String cannot fail");

    append_coverage_table(&mut body, "No-Hit Operations", coverage.get("no_hit"));
    append_coverage_table(
        &mut body,
        "Hit But No 2xx Operations",
        coverage.get("no_2xx"),
    );
    append_coverage_table(
        &mut body,
        "Side-Effecting Operations Hit But No 2xx",
        coverage.get("write_no_2xx"),
    );
    body
}

fn append_coverage_table(body: &mut String, title: &str, value: Option<&Value>) {
    writeln!(body, "## {title}").expect("writing to String cannot fail");
    writeln!(body).expect("writing to String cannot fail");
    let Some(entries) = value.and_then(Value::as_array) else {
        writeln!(body, "None.").expect("writing to String cannot fail");
        writeln!(body).expect("writing to String cannot fail");
        return;
    };
    if entries.is_empty() {
        writeln!(body, "None.").expect("writing to String cannot fail");
        writeln!(body).expect("writing to String cannot fail");
        return;
    }
    writeln!(
        body,
        "| Operation | Method | Path | Hits | Statuses | Latest Request |"
    )
    .expect("writing to String cannot fail");
    writeln!(body, "| --- | --- | --- | ---: | --- | --- |")
        .expect("writing to String cannot fail");
    for entry in entries {
        writeln!(
            body,
            "| `{}` | `{}` | `{}` | {} | `{}` | `{}` |",
            entry
                .get("operation_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            entry
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            entry
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            entry.get("hits").and_then(Value::as_u64).unwrap_or(0),
            entry
                .get("statuses")
                .map_or_else(|| "{}".to_string(), Value::to_string),
            entry
                .get("latest_request_id")
                .and_then(Value::as_str)
                .unwrap_or("")
        )
        .expect("writing to String cannot fail");
    }
    writeln!(body).expect("writing to String cannot fail");
}

fn match_request_record_to_operation(
    operations: &[OperationSummary],
    record: &Value,
) -> Option<usize> {
    if let Some(operation_id) = record.get("operation_id").and_then(Value::as_str)
        && let Some(index) = operations
            .iter()
            .position(|operation| operation.operation_id == operation_id)
    {
        return Some(index);
    }

    let method = record.get("method").and_then(Value::as_str)?;
    let path = record.get("path").and_then(Value::as_str)?;
    let path = path.split_once('?').map_or(path, |(path, _)| path);

    operations.iter().position(|operation| {
        operation.method.eq_ignore_ascii_case(method) && openapi_path_matches(&operation.path, path)
    })
}

pub(super) fn openapi_path_matches(openapi_path: &str, observed_path: &str) -> bool {
    if openapi_path == observed_path {
        return true;
    }
    let openapi_segments = openapi_path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let observed_segments = observed_path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if openapi_segments.len() != observed_segments.len() {
        return false;
    }
    openapi_segments
        .iter()
        .zip(observed_segments)
        .all(|(expected, observed)| {
            (expected.starts_with('{') && expected.ends_with('}')) || *expected == observed
        })
}

pub(super) fn list_operations(
    spec: &Value,
    filter: Option<&str>,
    method_filter: Option<HttpMethod>,
) -> Result<Vec<OperationSummary>, ApiCommandError> {
    let paths = spec
        .get("paths")
        .and_then(Value::as_object)
        .ok_or(ApiCommandError::MissingPaths)?;
    let filter = filter.map(str::to_lowercase);
    let mut operations = Vec::new();

    for (path, path_item) in paths {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };

        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
        ] {
            if method_filter.is_some_and(|wanted| wanted.openapi_key() != method.openapi_key()) {
                continue;
            }

            let Some(operation) = path_item.get(method.openapi_key()) else {
                continue;
            };

            let operation_id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .map_or_else(|| synthetic_operation_id(method, path), ToString::to_string);
            let summary = operation
                .get("summary")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let tags = operation
                .get("tags")
                .and_then(Value::as_array)
                .map(|tags| {
                    tags.iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            if let Some(filter) = &filter {
                let haystack = format!(
                    "{} {} {} {}",
                    operation_id,
                    path,
                    summary.as_deref().unwrap_or_default(),
                    tags.join(" ")
                )
                .to_lowercase();
                if !haystack.contains(filter) {
                    continue;
                }
            }

            let input_placeholders = operation_input_placeholders(path, operation);
            let requires_input = !input_placeholders.is_empty();
            let workflow_alternatives = workflow_alternatives(method, path);
            let raw_api_use =
                raw_api_use(method, path, operation, !workflow_alternatives.is_empty());
            operations.push(OperationSummary {
                inspect_command: format!("pcl api inspect {operation_id}"),
                call_command: example_call(method, path, operation),
                input_placeholders,
                requires_input,
                auth: operation_auth_metadata(method, path, operation),
                workflow_alternatives,
                raw_api_use,
                operation_id,
                method: method.as_str(),
                path: path.clone(),
                summary,
                tags,
            });
        }
    }

    operations.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.method.cmp(b.method))
            .then_with(|| a.operation_id.cmp(&b.operation_id))
    });

    Ok(operations)
}

pub(super) fn inspect_operation(
    spec: &Value,
    operation: &str,
    path: Option<&str>,
    full: bool,
) -> Result<Value, ApiCommandError> {
    let paths = spec
        .get("paths")
        .and_then(Value::as_object)
        .ok_or(ApiCommandError::MissingPaths)?;

    let operation_method = match operation.to_lowercase().as_str() {
        "get" => Some(HttpMethod::Get),
        "post" => Some(HttpMethod::Post),
        "put" => Some(HttpMethod::Put),
        "patch" => Some(HttpMethod::Patch),
        "delete" => Some(HttpMethod::Delete),
        _ => None,
    };

    if let (Some(method), Some(path)) = (operation_method, path) {
        let operation = paths
            .get(path)
            .and_then(|path_item| path_item.get(method.openapi_key()))
            .ok_or_else(|| {
                ApiCommandError::OperationNotFound(format!("{} {}", method.as_str(), path))
            })?;
        let operation_id = operation
            .get("operationId")
            .and_then(Value::as_str)
            .map_or_else(|| synthetic_operation_id(method, path), ToString::to_string);
        return Ok(operation_manifest(
            operation_id,
            method,
            path,
            operation,
            full,
        ));
    }

    for (candidate_path, path_item) in paths {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };

        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
        ] {
            let Some(candidate) = path_item.get(method.openapi_key()) else {
                continue;
            };
            let candidate_id = candidate
                .get("operationId")
                .and_then(Value::as_str)
                .map_or_else(
                    || synthetic_operation_id(method, candidate_path),
                    ToString::to_string,
                );
            if candidate_id == operation {
                return Ok(operation_manifest(
                    candidate_id,
                    method,
                    candidate_path,
                    candidate,
                    full,
                ));
            }
        }
    }

    Err(ApiCommandError::OperationNotFound(operation.to_string()))
}

fn operation_manifest(
    operation_id: String,
    method: HttpMethod,
    path: &str,
    operation: &Value,
    full: bool,
) -> Value {
    let workflow_alternatives = workflow_alternatives(method, path);
    let raw_api_use = raw_api_use(method, path, operation, !workflow_alternatives.is_empty());
    let mut manifest = json!({
        "operation_id": operation_id,
        "method": method.as_str(),
        "path": path,
        "summary": operation.get("summary").and_then(Value::as_str),
        "description": operation.get("description").and_then(Value::as_str),
        "auth": operation_auth_metadata(method, path, operation),
        "workflow_alternatives": workflow_alternatives,
        "raw_api_use": raw_api_use,
        "parameters": operation_parameters(operation),
        "path_params": named_parameters(operation, "path", false),
        "required_query": named_parameters(operation, "query", true),
        "request_body": request_body_manifest(operation),
        "body_fields": body_fields(operation),
        "body_variants": body_variants(operation),
        "required_body_fields": required_body_fields(operation),
        "body_template": openapi_body_template(operation),
        "input_placeholders": operation_input_placeholders(path, operation),
        "response_statuses": response_statuses(operation),
        "example_call": example_call(method, path, operation),
    });

    if full && let Some(object) = manifest.as_object_mut() {
        object.insert("operation".to_string(), operation.clone());
    }

    manifest
}

pub(super) fn workflow_alternatives(method: HttpMethod, path: &str) -> Vec<Value> {
    let mut alternatives = manifest_workflow_alternatives(method, path);
    alternatives.extend(special_workflow_alternatives(method, path));
    alternatives
}

fn manifest_workflow_alternatives(method: HttpMethod, path: &str) -> Vec<Value> {
    let Some(commands) = api_manifest()
        .get("commands")
        .and_then(Value::as_array)
        .cloned()
    else {
        return Vec::new();
    };

    let mut alternatives = Vec::new();
    let mut best_score = None;
    for command in commands {
        let Some(command_text) = command.get("command").and_then(Value::as_str) else {
            continue;
        };
        if command_text.starts_with("pcl api ") {
            continue;
        }
        let workflow = command_text
            .split_whitespace()
            .nth(1)
            .unwrap_or(command_text)
            .to_string();
        let Some(actions) = command.get("actions").and_then(Value::as_array) else {
            continue;
        };

        for action in actions {
            let Some(score) = manifest_action_match_score(action, method, path) else {
                continue;
            };
            match best_score {
                Some(best) if score < best => continue,
                Some(best) if score > best => {
                    alternatives.clear();
                    best_score = Some(score);
                }
                None => best_score = Some(score),
                Some(_) => {}
            }
            let action_name = action.get("name").and_then(Value::as_str);
            let example = workflow_example_for_operation(&workflow, action.get("example"), path);
            alternatives.push(json!({
                "workflow": workflow,
                "action": action_name,
                "command": command_text,
                "example": example,
                "required_flags": action.get("required_flags").cloned().unwrap_or(Value::Null),
                "body_template": action.get("body_template").cloned().unwrap_or(Value::Null),
            }));
        }
    }

    alternatives
}

fn workflow_example_for_operation(
    workflow: &str,
    example: Option<&Value>,
    operation_path: &str,
) -> Option<String> {
    let example = example.and_then(Value::as_str)?;
    if workflow == "integrations" {
        if operation_path.contains("/integrations/pagerduty") {
            return Some(example.replace("--provider slack", "--provider pagerduty"));
        }
        if operation_path.contains("/integrations/slack") {
            return Some(example.replace("--provider pagerduty", "--provider slack"));
        }
    }
    Some(example.to_string())
}

fn manifest_action_match_score(action: &Value, method: HttpMethod, path: &str) -> Option<usize> {
    let method_matches = action
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|action_method| action_method.eq_ignore_ascii_case(method.as_str()));
    if !method_matches {
        return None;
    }

    let action_path = action.get("path").and_then(Value::as_str)?;
    path_match_score(action_path, path)
}

fn path_match_score(pattern: &str, path: &str) -> Option<usize> {
    if pattern == path {
        return Some(usize::MAX);
    }
    let pattern_segments = path_segments(pattern);
    let path_segments = path_segments(path);
    if pattern_segments.len() != path_segments.len() {
        return None;
    }

    let mut score = 0;
    for (expected, observed) in pattern_segments.iter().zip(path_segments) {
        if is_path_placeholder(expected) {
            continue;
        }
        if *expected != observed {
            return None;
        }
        score += 1;
    }

    Some(score)
}

fn path_segments(path: &str) -> Vec<&str> {
    path.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn is_path_placeholder(segment: &str) -> bool {
    segment.starts_with('{') && segment.ends_with('}')
}

fn special_workflow_alternatives(method: HttpMethod, path: &str) -> Vec<Value> {
    let normalized_path = normalize_path_placeholders(path);
    match (method, normalized_path.as_str()) {
        (HttpMethod::Get, "/cli/auth/code") => {
            single_special_workflow(
                "auth",
                "login_challenge",
                "pcl auth login --no-wait --force --toon",
                "Device-login challenge is exposed as a structured auth command.",
            )
        }
        (HttpMethod::Get, "/cli/auth/status") => {
            single_special_workflow(
                "auth",
                "poll",
                "pcl auth poll --session-id <session-id> --device-secret <secret> --expires-at <rfc3339> --toon",
                "Polling is handled by the auth command returned in data.poll_command.",
            )
        }
        (HttpMethod::Post, "/cli/auth/verify") => {
            single_special_workflow(
                "auth",
                "verify",
                "pcl auth login --force --toon",
                "The login command owns verification and stores the resulting credentials.",
            )
        }
        (HttpMethod::Post, "/auth/refresh") => {
            single_special_workflow(
                "auth",
                "refresh",
                "pcl auth refresh --toon",
                "Refresh rotation is exposed as a structured auth command.",
            )
        }
        (HttpMethod::Get, "/openapi") => {
            single_special_workflow(
                "api",
                "manifest",
                "pcl api manifest --toon",
                "Use the CLI manifest/list/inspect surfaces for discovery instead of raw OpenAPI retrieval.",
            )
        }
        (HttpMethod::Get, "/projects") => {
            single_special_workflow(
                "projects",
                "explorer",
                "pcl projects --limit 10",
                "Project exploration uses the normalized project view endpoint.",
            )
        }
        (HttpMethod::Get, "/public/incidents") => {
            single_special_workflow(
                "incidents",
                "list_public",
                "pcl incidents --limit 5",
                "Public incident listing uses the normalized incident view endpoint.",
            )
        }
        (HttpMethod::Get, "/projects/{}/incidents") => {
            single_special_workflow(
                "incidents",
                "list_project",
                "pcl incidents --project <project-ref> --limit 50",
                "Project incident listing uses the normalized incident view endpoint.",
            )
        }
        (HttpMethod::Get, "/incidents/{}") => {
            single_special_workflow(
                "incidents",
                "detail",
                "pcl incidents --incident-id <incident-id>",
                "Incident detail uses the normalized incident view endpoint.",
            )
        }
        (HttpMethod::Get, "/incidents/{}/transactions/{}/trace") => {
            single_special_workflow(
                "incidents",
                "trace",
                "pcl incidents --incident-id <incident-id> --tx-id <tx-id>",
                "Incident traces use the normalized incident view endpoint.",
            )
        }
        (HttpMethod::Get, "/projects/{}/submitted-assertions") => {
            vec![
                special_workflow(
                    "releases",
                    "list",
                    "pcl releases list <project-ref>",
                    "Submitted assertions were superseded by release and registered-assertion workflows.",
                ),
                special_workflow(
                    "assertions",
                    "registered",
                    "pcl assertions --project <project-ref> --registered",
                    "Submitted assertions were superseded by release and registered-assertion workflows.",
                ),
            ]
        }
        (HttpMethod::Post, "/projects/{}/submitted-assertions") => {
            single_special_workflow(
                "releases",
                "create",
                "pcl apply --json",
                "Submitting assertions is now represented by creating a release through pcl apply or pcl releases.",
            )
        }
        _ => Vec::new(),
    }
}

fn single_special_workflow(workflow: &str, action: &str, example: &str, note: &str) -> Vec<Value> {
    vec![special_workflow(workflow, action, example, note)]
}

fn special_workflow(workflow: &str, action: &str, example: &str, note: &str) -> Value {
    json!({
        "workflow": workflow,
        "action": action,
        "example": example,
        "note": note,
    })
}

fn normalize_path_placeholders(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                "{}"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn raw_api_use(
    method: HttpMethod,
    path: &str,
    operation: &Value,
    has_workflow_alternative: bool,
) -> Value {
    if has_workflow_alternative {
        return json!({
            "policy": "prefer_workflow",
            "reason": "A first-class CLI workflow exists. Use raw api call only for debugging, OpenAPI parity checks, or reproducing low-level API behavior.",
        });
    }
    if service_api_key_raw_call_path(method, path) {
        return json!({
            "policy": "internal_service",
            "reason": "Service callback endpoint; normal CLI users and agents should not call it directly.",
        });
    }
    if requires_browser_session_token(path, operation) {
        return json!({
            "policy": "browser_session_bridge",
            "reason": "Browser/Privy session bridge; use auth/account commands for CLI authentication state.",
        });
    }

    json!({
        "policy": "debug_escape_hatch",
        "reason": "No first-class workflow is advertised; inspect first and preserve request IDs when using raw calls.",
    })
}

fn operation_parameters(operation: &Value) -> Vec<Value> {
    operation
        .get("parameters")
        .and_then(Value::as_array)
        .map(|parameters| {
            parameters
                .iter()
                .map(|parameter| {
                    json!({
                        "name": parameter.get("name").and_then(Value::as_str),
                        "in": parameter.get("in").and_then(Value::as_str),
                        "required": parameter.get("required").and_then(Value::as_bool).unwrap_or(false),
                        "schema": parameter.get("schema").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn named_parameters(operation: &Value, location: &str, required_only: bool) -> Vec<String> {
    operation
        .get("parameters")
        .and_then(Value::as_array)
        .map(|parameters| {
            parameters
                .iter()
                .filter(|parameter| parameter.get("in").and_then(Value::as_str) == Some(location))
                .filter(|parameter| {
                    !required_only
                        || parameter
                            .get("required")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                })
                .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn request_body_manifest(operation: &Value) -> Value {
    let Some(body) = operation.get("requestBody") else {
        return Value::Null;
    };
    json!({
        "required": body.get("required").and_then(Value::as_bool).unwrap_or(false),
        "content_types": body
            .get("content")
            .and_then(Value::as_object)
            .map(|content| content.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default(),
        "schema_type": body
            .pointer("/content/application~1json/schema")
            .map_or_else(|| "unknown".to_string(), compact_schema_type),
    })
}

fn body_schema(operation: &Value) -> Option<&Value> {
    operation.pointer("/requestBody/content/application~1json/schema")
}

pub(super) fn required_body_fields(operation: &Value) -> Vec<String> {
    body_schema(operation)
        .map(required_fields_for_schema)
        .unwrap_or_default()
}

fn required_fields_for_schema(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn body_fields(operation: &Value) -> Vec<Value> {
    body_schema(operation)
        .map(body_fields_for_schema)
        .unwrap_or_default()
}

fn body_fields_for_schema(schema: &Value) -> Vec<Value> {
    let required = required_fields_for_schema(schema);
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(name, schema)| {
                    json!({
                        "name": name,
                        "required": required.iter().any(|required| required == name),
                        "type": compact_schema_type(schema),
                        "enum": schema.get("enum").cloned().unwrap_or(Value::Null),
                        "const": schema.get("const").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn body_variants(operation: &Value) -> Vec<Value> {
    let Some(schema) = body_schema(operation) else {
        return Vec::new();
    };
    let Some(variants) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    variants
        .iter()
        .enumerate()
        .map(|(index, variant)| {
            json!({
                "name": schema_variant_name(variant, index),
                "schema_type": compact_schema_type(variant),
                "required_body_fields": required_fields_for_schema(variant),
                "body_fields": body_fields_for_schema(variant),
                "body_template": template_from_schema(variant),
            })
        })
        .collect()
}

fn schema_variant_name(schema: &Value, index: usize) -> String {
    schema
        .pointer("/properties/mode/const")
        .or_else(|| schema.pointer("/properties/mode/enum/0"))
        .and_then(Value::as_str)
        .map_or_else(|| format!("variant_{}", index + 1), ToString::to_string)
}

fn compact_schema_type(schema: &Value) -> String {
    if let Some(schema_type) = schema.get("type").and_then(Value::as_str) {
        return schema_type.to_string();
    }
    if schema.get("oneOf").is_some() {
        return "oneOf".to_string();
    }
    if schema.get("anyOf").is_some() {
        return "anyOf".to_string();
    }
    "unknown".to_string()
}

pub(super) fn openapi_body_template(operation: &Value) -> Value {
    let Some(schema) = body_schema(operation) else {
        return Value::Null;
    };
    template_from_schema(schema)
}

fn template_from_schema(schema: &Value) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut object = Map::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (name, property) in properties {
                    object.insert(name.clone(), template_from_schema(property));
                }
            }
            Value::Object(object)
        }
        Some("array") => {
            Value::Array(vec![
                schema
                    .get("items")
                    .map_or(Value::String("<item>".to_string()), template_from_schema),
            ])
        }
        Some("integer") | Some("number") => json!(0),
        Some("boolean") => json!(false),
        Some("string") => {
            if let Some(value) = schema.get("const") {
                return value.clone();
            }
            schema
                .get("enum")
                .and_then(Value::as_array)
                .and_then(|values| values.first())
                .cloned()
                .unwrap_or_else(|| Value::String("<string>".to_string()))
        }
        _ => {
            if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
                return options
                    .first()
                    .map_or(Value::String("<value>".to_string()), template_from_schema);
            }
            Value::String("<value>".to_string())
        }
    }
}

fn response_statuses(operation: &Value) -> Vec<Value> {
    operation
        .get("responses")
        .and_then(Value::as_object)
        .map(|responses| {
            responses
                .iter()
                .map(|(status, response)| {
                    json!({
                        "status": status,
                        "description": response.get("description").and_then(Value::as_str),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn example_call(method: HttpMethod, path: &str, operation: &Value) -> String {
    let path = example_path(path, operation);
    let mut command = format!(
        "pcl api call {} {}",
        method.openapi_key(),
        shell_quote_path(&path)
    );
    if should_allow_unauthenticated_raw_call(method, &path, operation) {
        command.push_str(" --allow-unauthenticated");
    }
    if service_api_key_raw_call_path(method, &path) {
        command.push_str(" --header 'x-api-key=<x-api-key>'");
    }
    for parameter in required_header_parameters(operation) {
        if service_api_key_raw_call_path(method, &path)
            && parameter.eq_ignore_ascii_case("x-api-key")
        {
            continue;
        }
        write!(
            command,
            " --header {}",
            shell_quote(&format!(
                "{parameter}={}",
                header_placeholder(&parameter, &path, operation)
            ))
        )
        .expect("writing to String cannot fail");
    }
    for parameter in required_query_parameters(operation) {
        write!(
            command,
            " --query {}",
            shell_quote(&format!("{parameter}=<{parameter}>"))
        )
        .expect("writing to String cannot fail");
    }
    if operation.get("requestBody").is_some() {
        let body = openapi_body_template(operation);
        if body.is_null() {
            command.push_str(" --body '{}'");
        } else {
            let body = serde_json::to_string(&body).unwrap_or_else(|_| "{...}".to_string());
            write!(command, " --body {}", shell_quote(&body))
                .expect("writing to String cannot fail");
        }
    }
    command
}

pub(super) fn operation_auth_metadata(method: HttpMethod, path: &str, operation: &Value) -> Value {
    let required_headers = required_header_parameters(operation);
    let browser_token_required = requires_browser_session_token(path, operation);
    let service_api_key_required = service_api_key_raw_call_path(method, path);
    let stored_cli_auth =
        !should_allow_unauthenticated_raw_call(method, path, operation) && !browser_token_required;
    let mut notes = Vec::new();
    if browser_token_required {
        notes.push(
            "Requires a browser/Privy session bearer token supplied with --header authorization=Bearer <privy-token>.",
        );
    }
    if stored_cli_auth {
        notes.push(
            "PCL attaches the stored CLI bearer token unless --allow-unauthenticated is set.",
        );
    }
    if service_api_key_required {
        notes.push("Requires a service API key supplied with --header x-api-key=<x-api-key>.");
    }
    json!({
        "stored_cli_auth": stored_cli_auth,
        "allow_unauthenticated_example": should_allow_unauthenticated_raw_call(method, path, operation),
        "browser_session_token_required": browser_token_required,
        "service_api_key_required": service_api_key_required,
        "required_headers": required_headers,
        "notes": notes,
    })
}

fn requires_browser_session_token(path: &str, operation: &Value) -> bool {
    path == "/web/auth/bootstrap-session" && has_required_authorization_parameter(operation)
}

fn header_placeholder(parameter: &str, path: &str, operation: &Value) -> String {
    if parameter.eq_ignore_ascii_case("authorization") {
        if requires_browser_session_token(path, operation) {
            return "Bearer <privy-token>".to_string();
        }
        return "Bearer <token>".to_string();
    }
    format!("<{parameter}>")
}

fn should_allow_unauthenticated_raw_call(
    method: HttpMethod,
    path: &str,
    operation: &Value,
) -> bool {
    service_api_key_raw_call_path(method, path)
        || (public_raw_call_path(method, path) && !has_required_authorization_parameter(operation))
}

fn service_api_key_raw_call_path(method: HttpMethod, path: &str) -> bool {
    method == HttpMethod::Post
        && (path.starts_with("/enforcer/")
            || path.starts_with("/indexer/")
            || path.starts_with("/tracer/")
            || path.starts_with("/backtesting/"))
}

pub(super) fn public_raw_call_path(method: HttpMethod, path: &str) -> bool {
    match method {
        HttpMethod::Get => {
            path == "/health"
                || path == "/cli/auth/code"
                || path == "/cli/auth/status"
                || path == "/openapi"
                || path == "/projects"
                || path == "/public/incidents"
                || path == "/stats"
                || path == "/system-status"
                || path == "/search"
                || path == "/assertions"
                || path == "/views/projects"
                || path.starts_with("/views/public/")
                || path.starts_with("/projects/resolve/")
                || path.starts_with("/web/verified-contract")
                || (path.starts_with("/invitations/") && path.ends_with("/preview"))
        }
        HttpMethod::Post => path == "/auth/refresh" || path == "/cli/auth/verify",
        HttpMethod::Put | HttpMethod::Patch | HttpMethod::Delete => false,
    }
}

fn has_required_authorization_parameter(operation: &Value) -> bool {
    required_header_parameters(operation)
        .iter()
        .any(|name| name.eq_ignore_ascii_case("authorization"))
}

fn example_path(path: &str, operation: &Value) -> String {
    let mut path = path.to_string();
    for parameter in named_parameters(operation, "path", false) {
        path = path.replace(&format!("{{{parameter}}}"), &format!("<{parameter}>"));
    }
    path
}

fn shell_quote_path(path: &str) -> String {
    if path.contains('<') || path.contains('>') {
        shell_quote(path)
    } else {
        path.to_string()
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn operation_input_placeholders(path: &str, operation: &Value) -> Vec<String> {
    let mut placeholders = named_parameters(operation, "path", false)
        .into_iter()
        .map(|parameter| format!("path:{parameter}"))
        .collect::<Vec<_>>();
    placeholders.extend(
        required_header_parameters(operation)
            .into_iter()
            .map(|parameter| format!("header:{parameter}")),
    );
    placeholders.extend(
        required_query_parameters(operation)
            .into_iter()
            .map(|parameter| format!("query:{parameter}")),
    );
    if operation.get("requestBody").is_some() {
        placeholders.push("body".to_string());
    }
    if placeholders.is_empty() && path.contains('{') {
        placeholders.push("path".to_string());
    }
    placeholders
}

fn required_header_parameters(operation: &Value) -> Vec<String> {
    named_parameters(operation, "header", true)
}

fn required_query_parameters(operation: &Value) -> Vec<String> {
    named_parameters(operation, "query", true)
}

pub(super) fn next_actions_for_operations(operations: &[OperationSummary]) -> Vec<String> {
    operations.first().map_or_else(
        || vec!["pcl api list".to_string(), "pcl api manifest".to_string()],
        |operation| {
            if let Some(example) = operation
                .workflow_alternatives
                .first()
                .and_then(|alternative| alternative.get("example"))
                .and_then(Value::as_str)
            {
                return vec![
                    example.to_string(),
                    format!("{} --toon", operation.inspect_command),
                ];
            }
            if operation.requires_input {
                vec![
                    format!("{} --toon", operation.inspect_command),
                    "Inspect the operation, then fill the placeholders in the example call"
                        .to_string(),
                ]
            } else {
                vec![
                    operation.inspect_command.clone(),
                    operation.call_command.clone(),
                ]
            }
        },
    )
}

pub(super) fn command_next_actions(inspected: &Value) -> Vec<String> {
    if let Some(example) = inspected
        .get("workflow_alternatives")
        .and_then(Value::as_array)
        .and_then(|alternatives| alternatives.first())
        .and_then(|alternative| alternative.get("example"))
        .and_then(Value::as_str)
    {
        return vec![example.to_string()];
    }
    inspected
        .get("example_call")
        .and_then(Value::as_str)
        .map_or_else(
            || vec!["pcl api list".to_string()],
            |command| vec![command.to_string()],
        )
}

pub(super) fn synthetic_operation_id(method: HttpMethod, path: &str) -> String {
    let mut id = method.openapi_key().to_string();
    let mut previous_was_separator = false;

    for ch in path.chars() {
        if ch.is_ascii_alphanumeric() {
            if previous_was_separator && !id.ends_with('_') {
                id.push('_');
            }
            id.push(ch.to_ascii_lowercase());
            previous_was_separator = false;
        } else {
            previous_was_separator = true;
        }
    }

    id.trim_end_matches('_').to_string()
}
