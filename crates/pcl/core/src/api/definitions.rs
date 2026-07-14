use crate::output::command_for_mode;
use pcl_common::args::OutputMode;
use serde_json::{
    Map,
    Value,
    json,
};

use super::{
    HttpMethod,
    generated_operation_template,
    workflows,
};

// Workflow metadata lives here so schema, manifest, OpenAPI alternatives, and
// output policies cannot silently diverge as new API functionality is added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkflowOutputPolicy {
    MachineRaw,
    MachineRawHumanCompactArtifacts,
}

impl WorkflowOutputPolicy {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::MachineRaw => "machine_raw",
            Self::MachineRawHumanCompactArtifacts => "machine_raw_human_compact_artifacts",
        }
    }
}

#[derive(Debug)]
pub(super) struct WorkflowDefinition {
    pub(super) name: &'static str,
    pub(super) command: &'static str,
    pub(in crate::api) description: &'static str,
    pub(in crate::api) output: &'static str,
    pub(super) output_policy: WorkflowOutputPolicy,
    pub(super) actions: &'static [WorkflowActionDefinition],
}

#[derive(Debug)]
pub(super) struct WorkflowActionDefinition {
    pub(super) name: &'static str,
    pub(super) auth: bool,
    pub(super) operation_id: &'static str,
    pub(in crate::api) required_flags: &'static [&'static str],
    pub(in crate::api) optional_flags: &'static [&'static str],
    pub(in crate::api) required_body_fields: &'static [&'static str],
    pub(in crate::api) body_template: Option<&'static str>,
    pub(in crate::api) query: &'static [(&'static str, &'static str)],
    pub(in crate::api) example: &'static str,
}

impl WorkflowDefinition {
    fn manifest_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("command".to_string(), json!(self.command));
        object.insert("description".to_string(), json!(self.description));
        object.insert("output".to_string(), json!(self.output));
        object.insert(
            "output_policy".to_string(),
            json!(self.output_policy.as_str()),
        );
        object.insert(
            "actions".to_string(),
            Value::Array(
                self.actions
                    .iter()
                    .map(WorkflowActionDefinition::manifest_value)
                    .collect(),
            ),
        );
        Value::Object(object)
    }
}

impl WorkflowActionDefinition {
    fn manifest_value(&self) -> Value {
        let method = self.method();
        let path = self.path();
        let mut object = Map::new();
        object.insert("name".to_string(), json!(self.name));
        object.insert("auth".to_string(), json!(self.auth));
        object.insert("operation_id".to_string(), json!(self.operation_id));
        object.insert("method".to_string(), json!(method.as_str()));
        object.insert("path".to_string(), json!(path));
        if !self.required_flags.is_empty() {
            object.insert(
                "required_flags".to_string(),
                string_array(self.required_flags),
            );
        }
        if !self.optional_flags.is_empty() {
            object.insert(
                "optional_flags".to_string(),
                string_array(self.optional_flags),
            );
        }
        if !self.required_body_fields.is_empty() {
            object.insert(
                "required_body_fields".to_string(),
                string_array(self.required_body_fields),
            );
        }
        if let Some(body_template) = self.body_template {
            object.insert("body_template".to_string(), json!(body_template));
        }
        if !self.query.is_empty() {
            object.insert("query".to_string(), query_object(self.query));
        }
        object.insert("example".to_string(), agent_command(self.example));
        Value::Object(object)
    }

    pub(super) fn method(&self) -> HttpMethod {
        self.generated_operation().0
    }

    pub(super) fn path(&self) -> &'static str {
        self.generated_operation().1
    }

    fn generated_operation(&self) -> (HttpMethod, &'static str) {
        generated_operation_template(self.operation_id).unwrap_or_else(|| {
            panic!(
                "workflow action `{}` references missing generated OpenAPI operation `{}`",
                self.name, self.operation_id
            )
        })
    }

    pub(super) fn required_flags_value(&self) -> Value {
        if self.required_flags.is_empty() {
            Value::Null
        } else {
            string_array(self.required_flags)
        }
    }

    pub(super) fn body_template_value(&self) -> Value {
        self.body_template.map_or(Value::Null, |value| json!(value))
    }

    pub(super) fn example_for_operation(&self, workflow: &str, operation_path: &str) -> String {
        let mut example = agent_command(self.example)
            .as_str()
            .unwrap_or_default()
            .to_string();
        if workflow == "integrations" {
            if operation_path.contains("/integrations/pagerduty") {
                example = example.replace("--provider slack", "--provider pagerduty");
            } else if operation_path.contains("/integrations/slack") {
                example = example.replace("--provider pagerduty", "--provider slack");
            }
        }
        example
    }
}

pub(super) fn workflow_definitions() -> &'static [WorkflowDefinition] {
    WORKFLOW_DEFINITIONS
}

pub(super) fn workflow_definition(name: &str) -> Option<&'static WorkflowDefinition> {
    WORKFLOW_DEFINITIONS
        .iter()
        .find(|definition| definition.name == name)
}

pub(super) fn workflow_output_policy(name: &str) -> WorkflowOutputPolicy {
    workflow_definition(name).map_or(WorkflowOutputPolicy::MachineRaw, |definition| {
        definition.output_policy
    })
}

pub(super) fn command_manifests() -> Vec<Value> {
    WORKFLOW_DEFINITIONS
        .iter()
        .map(WorkflowDefinition::manifest_value)
        .chain(raw_api_command_manifests())
        .collect()
}

pub(super) fn agent_examples() -> Vec<Value> {
    [
        "pcl incidents --limit 5",
        "pcl search --query settler",
        "pcl releases list <project-ref>",
        "pcl access members <project-ref>",
        "pcl integrations --project <project-ref> --provider slack",
        "pcl api list --filter incidents",
    ]
    .into_iter()
    .map(agent_command)
    .collect()
}

fn raw_api_command_manifests() -> Vec<Value> {
    vec![
        json!({
            "command": "pcl api manifest",
            "description": "Print this agent-readable command manifest.",
        }),
        json!({
            "command": "pcl api list [--filter <term>] [--method <get|post|put|patch|delete>]",
            "description": "List OpenAPI operations with executable inspect and call commands.",
            "output": "operations[] with operation_id, method, path, summary, tags, workflow_alternatives, raw_api_use, inspect_command, call_command",
        }),
        json!({
            "command": "pcl api inspect <operation_id>|<method> <path> [--full]",
            "description": "Inspect a compact operation manifest. Use --full for raw OpenAPI.",
            "output": "operation_id, method, path, auth metadata, workflow_alternatives, raw_api_use, path_params, required_query, body_fields, required_body_fields, body_template, response_statuses, example_call",
        }),
        json!({
            "command": "pcl api call <method> <path[?query]> [--query key=value] [--field key=value] [--body-file body.json] [--paginate <field>] [--page-param page] [--limit-param limit] [--jsonl] [--output <file>]",
            "description": "Execute any endpoint below /api/v1. Query strings in PATH and repeated --query flags are both accepted; --field merges simple JSON object body fields; GET calls can paginate any array response with --paginate.",
            "output": "request and response status/body; non-2xx responses return structured error envelopes with request_id when the API provides one. Raw calls log operation_id when the live OpenAPI manifest can resolve the method/path.",
            "actions": [
                {"name": "execute", "method": "*", "path": "<path>", "auth": "default", "example": agent_command("pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated")},
                {"name": "paginate", "method": "GET", "path": "<path>", "auth": "default", "required_flags": ["--paginate"], "optional_flags": ["--all", "--page", "--limit", "--page-param", "--limit-param", "--max-pages", "--jsonl", "--output"], "example": agent_command("pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --output incidents.json")},
                {"name": "export_jsonl", "method": "GET", "path": "<path>", "auth": "default", "required_flags": ["--paginate", "--jsonl", "--output"], "example": agent_command("pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --jsonl --output incidents.jsonl")}
            ],
        }),
        json!({
            "command": "pcl api coverage [--records <n>] [--markdown <path>]",
            "description": "Audit local request history against the live OpenAPI surface. Old records are matched by method/path; new raw api calls also persist operation_id.",
            "output": "total operations, by-method coverage, no-hit operations, hit-but-no-2xx operations, side-effecting no-2xx operations, unmatched records",
        }),
    ]
}

fn string_array(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|value| json!(value)).collect())
}

fn query_object(values: &[(&str, &str)]) -> Value {
    Value::Object(
        values
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

fn agent_command(command: &str) -> Value {
    json!(command_for_mode(command, OutputMode::Json))
}

const WORKFLOW_DEFINITIONS: &[WorkflowDefinition] = &[
    workflows::incidents::DEFINITION,
    workflows::projects::DEFINITION,
    workflows::assertions::DEFINITION,
    workflows::search::DEFINITION,
    workflows::account::DEFINITION,
    workflows::contracts::DEFINITION,
    workflows::releases::DEFINITION,
    workflows::deployments::DEFINITION,
    workflows::access::DEFINITION,
    workflows::integrations::DEFINITION,
    workflows::protocol_manager::DEFINITION,
    workflows::events::DEFINITION,
];
