use serde_json::json;

use super::{
    definitions::{
        agent_examples,
        command_manifests,
    },
    spec::workflow_spec_summary,
};

pub fn api_manifest() -> serde_json::Value {
    json!({
        "name": "pcl",
        "description": "Use top-level workflow commands for product workflows; use pcl api list/inspect/call only for debugging, API parity checks, internal/service endpoints, or endpoints not yet promoted to a workflow.",
        "raw_api": "pcl api list | pcl api inspect | pcl api call | pcl api coverage | pcl api manifest",
        "raw_api_policy": {
            "normal_work": "Use workflow_alternatives from pcl api list/inspect when present, or start with pcl workflows and pcl schema.",
            "allowed_uses": ["debugging", "OpenAPI parity checks", "service/internal endpoint investigation", "browser-session bridge investigation", "new endpoint exploration before promotion"],
            "not_normal_path": "Agents should not call raw endpoints for incidents, projects, assertions, releases, integrations, access, protocol-manager, events, search, or auth when a workflow alternative is advertised."
        },
        "llms": "pcl --json --llms | pcl llms --json",
        "default_output": "human",
        "output_modes": {
            "default": "Human-readable output optimized for people.",
            "json": "Pass --json for the {status,data,error,next_actions} envelope as JSON."
        },
        "body_input": {
            "preferred": "Use typed flags when available, then --field key=value, then --body-file for nested payloads.",
            "template_flag": "--body-template",
            "field_flag": "--field key=value parses JSON scalars/objects/arrays when VALUE is valid JSON, otherwise a string"
        },
        "workflow_specs": workflow_spec_summary(),
        "pagination": {
            "workflow": "Use workflow-specific --all where available, for example pcl incidents --all --limit 50 --output incidents.json.",
            "raw_call": "Use pcl api call get /path --paginate <array-field> --limit 50 --max-pages 100 --output results.json for generic GET pagination.",
            "jsonl": "Add --jsonl with --output on paginated commands to write one item per line for resumable analysis."
        },
        "auth": {
            "default": "Stored bearer token is attached only when the selected operation requires auth; known public raw paths do not attach stale local tokens.",
            "public_endpoints": "Workflow commands use public view endpoints without requiring login when possible.",
            "login_command": "pcl auth login",
        },
        "safety": {
            "body_templates": "Use --body-template before mutation workflows that require nested payloads.",
            "execution": "Workflow commands execute when invoked. Use typed flags first, then --field key=value or --body-file body.json for request bodies."
        },
        "product_surfaces": [
            {"command": "pcl --json --llms | pcl llms --json", "description": "Print the CLI-native LLM usage guide for agents."},
            {"command": "pcl doctor --json", "description": "Diagnose config, auth, request-log, artifact, and API health state."},
            {"command": "pcl whoami --json", "description": "Print local identity, token validity, and expiry."},
            {"command": "pcl workflows [show <name>] --json", "description": "List agent-friendly workflow recipes with concrete command steps."},
            {"command": "pcl export incidents --json", "description": "Export incident list data as resumable JSONL artifacts with checkpoint and error files."},
            {"command": "pcl artifacts [path|init|list] --json", "description": "Find and inspect generated artifacts."},
            {"command": "pcl jobs [path|list|status|resume|cancel] --json", "description": "Inspect resumable local job records from export workflows."},
            {"command": "pcl requests|logs [path|list|clear] --json", "description": "Inspect the local API request log with status and request IDs."},
            {"command": "pcl api coverage [--records <n>] [--markdown <path>] --json", "description": "Compare the local request log with the live OpenAPI manifest and report hit/no-hit/no-2xx coverage."},
            {"command": "pcl schema [list|get <workflow>] --json", "description": "Inspect workflow/action schemas from the command manifest."},
            {"command": "pcl completions <shell>", "description": "Print raw shell completion scripts for bash, zsh, fish, powershell, and elvish. Use --json only when an installer expects the script inside an envelope."}
        ],
        "commands": command_manifests(),
        "examples": agent_examples(),
    })
}
