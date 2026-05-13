use serde_json::{
    Value,
    json,
};

use super::spec::workflow_spec_summary;

pub fn api_manifest() -> Value {
    json!({
        "name": "pcl",
        "description": "Use top-level workflow commands for product workflows; use pcl api list/inspect/call only for debugging, API parity checks, internal/service endpoints, or endpoints not yet promoted to a workflow.",
        "raw_api": "pcl api list | pcl api inspect | pcl api call | pcl api coverage | pcl api manifest",
        "raw_api_policy": {
            "normal_work": "Use workflow_alternatives from pcl api list/inspect when present, or start with pcl workflows and pcl schema.",
            "allowed_uses": ["debugging", "OpenAPI parity checks", "service/internal endpoint investigation", "browser-session bridge investigation", "new endpoint exploration before promotion"],
            "not_normal_path": "Agents should not call raw endpoints for incidents, projects, assertions, releases, integrations, access, protocol-manager, transfers, events, search, or auth when a workflow alternative is advertised."
        },
        "llms": "pcl --toon --llms | pcl llms --toon",
        "default_output": "human",
        "output_modes": {
            "default": "Human-readable output optimized for people.",
            "toon": "Pass --toon for compact machine-readable envelopes.",
            "json": "Pass --json for the same {status,data,error,next_actions} envelope as JSON."
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
            "dry_run": "Optional planning mode: add --dry-run to workflow commands before write flags, for example `pcl projects --dry-run --create ...`. Re-run without --dry-run only when ready to execute.",
            "destructive_detection": "Request plans flag likely destructive paths, but raw api call does not enforce a confirmation gate."
        },
        "product_surfaces": [
            {"command": "pcl --toon --llms | pcl llms --toon", "description": "Print the CLI-native LLM usage guide for agents."},
            {"command": "pcl doctor --toon", "description": "Diagnose config, auth, request-log, artifact, and API health state."},
            {"command": "pcl whoami --toon", "description": "Print local identity, token validity, and expiry."},
            {"command": "pcl workflows [show <name>] --toon", "description": "List agent-friendly workflow recipes with concrete command steps."},
            {"command": "pcl export incidents --toon", "description": "Export incident list data as resumable JSONL artifacts with checkpoint and error files."},
            {"command": "pcl artifacts [path|init|list] --toon", "description": "Find and inspect generated artifacts."},
            {"command": "pcl jobs [path|list|status|resume|cancel] --toon", "description": "Inspect resumable local job records from export workflows."},
            {"command": "pcl requests|logs [path|list|clear] --toon", "description": "Inspect the local API request log with status and request IDs."},
            {"command": "pcl api coverage [--records <n>] [--markdown <path>] --toon", "description": "Compare the local request log with the live OpenAPI manifest and report hit/no-hit/no-2xx coverage."},
            {"command": "pcl schema [list|get <workflow>] --toon", "description": "Inspect workflow/action schemas from the command manifest."},
            {"command": "pcl completions <shell> --toon", "description": "Generate shell completion scripts for bash, zsh, fish, powershell, and elvish."}
        ],
        "commands": [
            {
                "command": "pcl incidents [--project-id <id>] [--incident-id <id>] [--stats] [--limit <n>] [--all --output <file>]",
                "description": "List public incidents, project incidents, fetch all incident pages, inspect incident detail, incident stats, or incident trace.",
                "output": "incident data from /views/public/incidents, /views/projects/{projectId}/incidents, /views/incidents/{incidentId}, or /projects/{project_id}/incidents/stats",
                "actions": [
                    {"name": "list_public", "auth": false, "method": "GET", "path": "/views/public/incidents", "optional_flags": ["--page", "--limit", "--network", "--sort", "--dev-mode", "--all", "--max-pages", "--output"], "example": "pcl incidents --limit 5"},
                    {"name": "list_project", "auth": true, "method": "GET", "path": "/views/projects/{projectId}/incidents", "required_flags": ["--project"], "optional_flags": ["--page", "--limit", "--assertion-id", "--adopter-id", "--environment", "--from", "--to", "--all", "--max-pages", "--output"], "example": "pcl incidents --project <project-ref> --all --limit 50 --output incidents.json"},
                    {"name": "stats", "auth": true, "method": "GET", "path": "/projects/{project_id}/incidents/stats", "required_flags": ["--project"], "example": "pcl incidents --project <project-ref> --stats"},
                    {"name": "detail", "auth": true, "method": "GET", "path": "/views/incidents/{incidentId}", "required_flags": ["--incident-id"], "example": "pcl incidents --incident-id <incident-id>"},
                    {"name": "trace", "auth": true, "method": "GET", "path": "/views/incidents/{incidentId}/transactions/{txId}/trace", "required_flags": ["--incident-id", "--tx-id"], "example": "pcl incidents --incident-id <incident-id> --tx-id <invalidating-transaction-id>"},
                    {"name": "retry_trace", "auth": true, "method": "POST", "path": "/incidents/{incident_id}/transactions/{tx_id}/trace/retry", "required_flags": ["--incident-id", "--tx-id"], "body_template": "empty_object", "example": "pcl incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace"}
                ]
            },
            {
                "command": "pcl projects <list|mine|show|saved|create|update|delete|save|unsave|resolve|widget>",
                "description": "List, inspect, create, update, save, unsave, resolve, widget, and delete projects.",
                "output": "project explorer, your projects, project detail, saved projects, widget, or mutation result",
                "legacy_examples": ["pcl projects --mine", "pcl projects --project <project-ref>", "pcl projects --create --project-name demo --chain-id 1"],
                "actions": [
                    {"name": "explorer", "auth": false, "method": "GET", "path": "/views/projects", "example": "pcl projects list --limit 10"},
                    {"name": "mine", "auth": true, "method": "GET", "path": "/views/projects/home", "example": "pcl projects mine", "legacy_aliases": ["pcl projects --mine", "pcl projects --home"]},
                    {"name": "saved", "auth": true, "method": "GET", "path": "/projects/saved", "required_flags": ["--user-id"], "query": {"user_id": "<user-id>"}, "example": "pcl projects saved --user-id <user-id>"},
                    {"name": "detail", "auth": true, "method": "GET", "path": "/projects/{project_id}", "required_flags": ["<project-ref>"], "example": "pcl projects show <project-ref>"},
                    {"name": "create", "auth": true, "method": "POST", "path": "/projects", "body_template": "project_create", "required_body_fields": ["project_name", "chain_id"], "example": "pcl projects create --project-name demo --chain-id 1"},
                    {"name": "update", "auth": true, "method": "PUT", "path": "/projects/{project_id}", "required_flags": ["<project-ref>"], "body_template": "project_update", "example": "pcl projects update <project-ref> --field github_url=https://github.com/org/repo"},
                    {"name": "delete", "auth": true, "method": "DELETE", "path": "/projects/{project_id}", "required_flags": ["<project-ref>"], "example": "pcl projects delete <project-ref>"},
                    {"name": "save", "auth": true, "method": "POST", "path": "/projects/saved", "required_flags": ["<project-ref>"], "body_template": "project_saved", "example": "pcl projects save <project-ref>"},
                    {"name": "unsave", "auth": true, "method": "DELETE", "path": "/projects/saved", "required_flags": ["<project-ref>"], "body_template": "project_saved", "example": "pcl projects unsave <project-ref>"},
                    {"name": "resolve", "auth": false, "method": "GET", "path": "/projects/resolve/{project_ref}", "required_flags": ["<project-ref>"], "example": "pcl projects resolve <project-ref>"},
                    {"name": "widget", "auth": true, "method": "GET", "path": "/projects/{project_id}/widget", "required_flags": ["<project-ref>"], "example": "pcl projects widget <project-ref>"}
                ]
            },
            {
                "command": "pcl assertions --project <ref> [--assertion-id <id>|--registered|--remove-info|--remove-calldata]",
                "description": "List, inspect, and manage project assertion lifecycle state.",
                "output": "assertion index/detail, registered assertions, or removal info/calldata",
                "actions": [
                    {"name": "index", "auth": true, "method": "GET", "path": "/views/projects/{projectId}/assertions", "required_flags": ["--project"], "example": "pcl assertions --project <project-ref>"},
                    {"name": "detail", "auth": true, "method": "GET", "path": "/views/projects/{projectId}/assertions/{assertionId}", "required_flags": ["--project", "--assertion-id"], "example": "pcl assertions --project <project-ref> --assertion-id <assertion-id>"},
                    {"name": "adopter_lookup", "auth": false, "method": "GET", "path": "/assertions", "required_flags": ["--adopter-address"], "optional_flags": ["--network", "--environment", "--include-onchain-only"], "example": "pcl assertions --adopter-address 0x... --network 1"},
                    {"name": "registered", "auth": true, "method": "GET", "path": "/projects/{project_id}/registered-assertions", "required_flags": ["--project"], "example": "pcl assertions --project <project-ref> --registered"},
                    {"name": "remove_info", "auth": true, "method": "GET", "path": "/projects/{project_id}/remove-assertions-info", "required_flags": ["--project"], "example": "pcl assertions --project <project-ref> --remove-info"},
                    {"name": "remove_calldata", "auth": true, "method": "GET", "path": "/projects/{project_id}/remove-assertions-calldata", "required_flags": ["--project"], "example": "pcl assertions --project <project-ref> --remove-calldata"}
                ]
            },
            {
                "command": "pcl search [--query <term>] [--stats] [--system-status] [--verified-contract --address <addr> --chain-id <id>]",
                "description": "Search projects/contracts and inspect platform metadata.",
                "output": "search results, stats, system status, health, whitelist, or verified contract data",
                "actions": [
                    {"name": "query", "auth": false, "method": "GET", "path": "/search", "optional_flags": ["--query"], "example": "pcl search --query settler"},
                    {"name": "stats", "auth": false, "method": "GET", "path": "/stats", "example": "pcl search --stats"},
                    {"name": "system_status", "auth": false, "method": "GET", "path": "/system-status", "example": "pcl search --system-status"},
                    {"name": "health", "auth": false, "method": "GET", "path": "/health", "example": "pcl search --health"},
                    {"name": "whitelist", "auth": true, "method": "GET", "path": "/whitelist", "example": "pcl search --whitelist"},
                    {"name": "verified_contract", "auth": false, "method": "GET", "path": "/web/verified-contract", "required_flags": ["--address", "--chain-id"], "example": "pcl search --verified-contract --address 0x... --chain-id 1"}
                ]
            },
            {
                "command": "pcl account [--me|--accept-terms|--logout]",
                "description": "Inspect authenticated web user state and perform onboarding actions.",
                "output": "current user account state, terms acceptance result, or logout result",
                "actions": [
                    {"name": "me", "auth": true, "method": "GET", "path": "/web/auth/me", "example": "pcl account"},
                    {"name": "accept_terms", "auth": true, "method": "POST", "path": "/web/auth/accept-terms", "body_template": "empty_object", "example": "pcl account --accept-terms"},
                    {"name": "logout", "auth": true, "method": "POST", "path": "/web/auth/logout", "body_template": "empty_object", "example": "pcl account --logout"}
                ]
            },
            {
                "command": "pcl contracts [--project <ref>] [--adopter-id <id>] [--unassigned --manager <address>] [--create --body-template]",
                "description": "List and manage project contracts and assertion adopters.",
                "output": "contract views, adopter records, assignment results, or remove calldata",
                "actions": [
                    {"name": "list_all", "auth": true, "method": "GET", "path": "/assertion_adopters", "example": "pcl contracts"},
                    {"name": "list_project", "auth": true, "method": "GET", "path": "/views/projects/{project}/contracts", "required_flags": ["--project"], "example": "pcl contracts --project <project-ref>"},
                    {"name": "detail", "auth": true, "method": "GET", "path": "/views/projects/{project}/contracts/{adopter_id}", "required_flags": ["--project", "--adopter-id"], "example": "pcl contracts --project <project-ref> --adopter-id <adopter-id>"},
                    {"name": "unassigned", "auth": true, "method": "GET", "path": "/assertion_adopters/no-project", "required_flags": ["--manager"], "query": {"manager": "<manager-address>"}, "example": "pcl contracts --unassigned --manager 0x..."},
                    {"name": "create", "auth": true, "method": "POST", "path": "/assertion_adopters", "body_template": "contracts", "example": "pcl contracts --create --body-template"},
                    {"name": "assign_project", "auth": true, "method": "POST", "path": "/assertion_adopters/assign-project", "body_template": "contracts_assign_project", "example": "pcl contracts --assign-project --body-template"},
                    {"name": "remove", "auth": true, "method": "DELETE", "path": "/projects/{project}/{aa_address}", "required_flags": ["--project", "--aa-address"], "example": "pcl contracts --project <project-ref> --aa-address 0x... --remove"},
                    {"name": "remove_calldata", "auth": true, "method": "GET", "path": "/assertion_adopters/{aa_address}/remove-assertions-calldata", "required_flags": ["--aa-address", "--assertion-id"], "optional_flags": ["--network", "--environment"], "query": {"assertion_ids": "<assertion-id>", "network": "<chain-id>", "environment": "production|staging"}, "example": "pcl contracts --aa-address 0x... --remove-calldata --network 1 --assertion-id 0x..."}
                ]
            },
            {
                "command": "pcl releases <list|show|create|preview|deploy|remove|calldata|backtest-progress|retry-check>",
                "description": "List, inspect, create, preview, deploy, check progress, retry failed checks, and remove releases.",
                "output": "release data, diffs, check progress, deployment confirmations, or calldata",
                "legacy_examples": ["pcl releases --project <project-ref>", "pcl releases --project <project-ref> --release-id <release-id>", "pcl releases --project <project-ref> --preview --body-file release.json"],
                "actions": [
                    {"name": "list", "auth": true, "method": "GET", "path": "/projects/{project}/releases", "required_flags": ["<project-ref>"], "example": "pcl releases list <project-ref>"},
                    {"name": "detail", "auth": true, "method": "GET", "path": "/projects/{project}/releases/{release_id}", "required_flags": ["<project-ref>", "<release-id>"], "example": "pcl releases show <project-ref> <release-id>"},
                    {"name": "preview", "auth": true, "method": "POST", "path": "/projects/{project}/releases/preview", "required_flags": ["<project-ref>"], "body_template": "release", "example": "pcl releases preview <project-ref> --body-file release.json"},
                    {"name": "create", "auth": true, "method": "POST", "path": "/projects/{project}/releases", "required_flags": ["<project-ref>"], "body_template": "release", "example": "pcl releases create <project-ref> --body-file release.json"},
                    {"name": "backtest_progress", "auth": true, "method": "GET", "path": "/projects/{project}/releases/{release_id}/backtest-progress", "required_flags": ["<project-ref>", "<release-id>"], "example": "pcl releases backtest-progress <project-ref> <release-id>"},
                    {"name": "retry_check", "auth": true, "method": "POST", "path": "/projects/{project}/releases/{release_id}/checks/{check_id}/retry", "required_flags": ["<project-ref>", "<release-id>", "<check-id>"], "body_template": "empty_object", "example": "pcl releases retry-check <project-ref> <release-id> <check-id>"},
                    {"name": "deploy_calldata", "auth": true, "method": "GET", "path": "/projects/{project}/releases/{release_id}/deploy-calldata", "required_flags": ["<project-ref>", "<release-id>", "--signer-address"], "query": {"signerAddress": "<signer-address>"}, "example": "pcl releases calldata deploy <project-ref> <release-id> --signer-address 0x..."},
                    {"name": "deploy", "auth": true, "method": "POST", "path": "/projects/{project}/releases/{release_id}/deploy", "required_flags": ["<project-ref>", "<release-id>"], "body_template": "release_deploy", "example": "pcl releases deploy <project-ref> <release-id> --body-template"},
                    {"name": "remove_calldata", "auth": true, "method": "GET", "path": "/projects/{project}/releases/{release_id}/remove-calldata", "required_flags": ["<project-ref>", "<release-id>"], "example": "pcl releases calldata remove <project-ref> <release-id>"},
                    {"name": "remove", "auth": true, "method": "POST", "path": "/projects/{project}/releases/{release_id}/remove", "required_flags": ["<project-ref>", "<release-id>"], "body_template": "release_remove", "example": "pcl releases remove <project-ref> <release-id> --body-template"}
                ]
            },
            {
                "command": "pcl deployments --project <ref> [--confirm --body-template]",
                "description": "Inspect deployment state and confirm deployed assertions.",
                "output": "deployment view or confirmation result",
                "actions": [
                    {"name": "list", "auth": true, "method": "GET", "path": "/views/projects/{project}/deployments", "required_flags": ["--project"], "example": "pcl deployments --project <project-ref>"},
                    {"name": "confirm", "auth": true, "method": "POST", "path": "/projects/{project}/confirm-deployment", "required_flags": ["--project"], "body_template": "deployment_confirmation", "example": "pcl deployments --project <project-ref> --confirm --body-template"}
                ]
            },
            {
                "command": "pcl access <members|invitations|pending|preview|accept|invite|resend|revoke|role|member|my-role>",
                "description": "Manage project members, roles, and invitations.",
                "output": "member lists, invitation lists, role data, or mutation results",
                "legacy_examples": ["pcl access --project <project-ref> --members", "pcl access --project <project-ref> --invite --body-template", "pcl access --token <token> --preview"],
                "actions": [
                    {"name": "members", "auth": true, "method": "GET", "path": "/projects/{project}/members", "required_flags": ["<project-ref>"], "example": "pcl access members <project-ref>"},
                    {"name": "my_role", "auth": true, "method": "GET", "path": "/projects/{project}/my-role", "required_flags": ["<project-ref>"], "example": "pcl access my-role <project-ref>"},
                    {"name": "invitations", "auth": true, "method": "GET", "path": "/projects/{project}/invitations", "required_flags": ["<project-ref>"], "example": "pcl access invitations <project-ref>"},
                    {"name": "invite", "auth": true, "method": "POST", "path": "/projects/{project}/invitations", "required_flags": ["<project-ref>"], "body_template": "access_invite", "example": "pcl access invite <project-ref> --body-template"},
                    {"name": "resend", "auth": true, "method": "POST", "path": "/projects/{project}/invitations/{invitation_id}/resend", "required_flags": ["<project-ref>", "<invitation-id>"], "body_template": "empty_object", "example": "pcl access resend <project-ref> <invitation-id>"},
                    {"name": "revoke", "auth": true, "method": "DELETE", "path": "/projects/{project}/invitations/{invitation_id}", "required_flags": ["<project-ref>", "<invitation-id>"], "body_template": "empty_object", "example": "pcl access revoke <project-ref> <invitation-id>"},
                    {"name": "update_role", "auth": true, "method": "PATCH", "path": "/projects/{project}/members/{member_user_id}", "required_flags": ["<project-ref>", "<member-user-id>"], "body_template": "role_update", "example": "pcl access role update <project-ref> <member-user-id> --body-template"},
                    {"name": "remove", "auth": true, "method": "DELETE", "path": "/projects/{project}/members/{member_user_id}", "required_flags": ["<project-ref>", "<member-user-id>"], "body_template": "empty_object", "example": "pcl access member remove <project-ref> <member-user-id>"},
                    {"name": "pending", "auth": true, "method": "GET", "path": "/invitations/pending", "example": "pcl access pending"},
                    {"name": "preview", "auth": false, "method": "GET", "path": "/invitations/{token}/preview", "required_flags": ["<token>"], "example": "pcl access preview <token>"},
                    {"name": "accept", "auth": true, "method": "POST", "path": "/invitations/{token}/accept", "required_flags": ["<token>"], "body_template": "empty_object", "example": "pcl access accept <token>"}
                ]
            },
            {
                "command": "pcl integrations --project <ref> --provider <slack|pagerduty> [--configure|--test|--delete]",
                "description": "Manage Slack and PagerDuty integrations.",
                "output": "integration status or mutation/test results",
                "actions": [
                    {"name": "get", "auth": true, "method": "GET", "path": "/projects/{project}/integrations/{provider}", "required_flags": ["--project", "--provider"], "example": "pcl integrations --project <project-ref> --provider slack"},
                    {"name": "configure", "auth": true, "method": "POST", "path": "/projects/{project}/integrations/{provider}", "required_flags": ["--project", "--provider"], "body_template": "slack|pagerduty", "example": "pcl integrations --project <project-ref> --provider slack --configure --body-template"},
                    {"name": "test", "auth": true, "method": "POST", "path": "/projects/{project}/integrations/{provider}/test", "required_flags": ["--project", "--provider"], "body_template": "slack|pagerduty", "example": "pcl integrations --project <project-ref> --provider slack --test"},
                    {"name": "delete", "auth": true, "method": "DELETE", "path": "/projects/{project}/integrations/{provider}", "required_flags": ["--project", "--provider"], "example": "pcl integrations --project <project-ref> --provider slack --delete"}
                ]
            },
            {
                "command": "pcl protocol-manager --project <ref> [--nonce --address <address>|--set|--clear|--transfer-calldata|--accept-calldata|--pending-transfer|--confirm-transfer]",
                "description": "Manage protocol manager transfers and calldata.",
                "output": "manager state, nonce, calldata, pending transfer, or mutation result",
                "actions": [
                    {"name": "pending_transfer", "auth": true, "method": "GET", "path": "/projects/{project}/protocol-manager/pending-transfer", "required_flags": ["--project"], "example": "pcl protocol-manager --project <project-ref> --pending-transfer"},
                    {"name": "nonce", "auth": true, "method": "GET", "path": "/projects/{project}/protocol-manager/nonce", "required_flags": ["--project", "--address"], "optional_flags": ["--chain-id"], "query": {"address": "<address>", "chain_id": "<chain-id>"}, "example": "pcl protocol-manager --project <project-ref> --nonce --address 0x..."},
                    {"name": "set", "auth": true, "method": "POST", "path": "/projects/{project}/protocol-manager", "required_flags": ["--project"], "body_template": "protocol_manager_set", "example": "pcl protocol-manager --project <project-ref> --set --body-template"},
                    {"name": "clear", "auth": true, "method": "DELETE", "path": "/projects/{project}/protocol-manager", "required_flags": ["--project"], "body_template": "empty_object", "example": "pcl protocol-manager --project <project-ref> --clear"},
                    {"name": "transfer_calldata", "auth": true, "method": "GET", "path": "/projects/{project}/protocol-manager/transfer-calldata", "required_flags": ["--project", "--new-manager"], "query": {"new_manager": "<address>"}, "example": "pcl protocol-manager --project <project-ref> --transfer-calldata --new-manager 0x..."},
                    {"name": "accept_calldata", "auth": true, "method": "GET", "path": "/projects/{project}/protocol-manager/accept-calldata", "required_flags": ["--project"], "example": "pcl protocol-manager --project <project-ref> --accept-calldata"},
                    {"name": "confirm_transfer", "auth": true, "method": "POST", "path": "/projects/{project}/protocol-manager/confirm-transfer", "required_flags": ["--project"], "body_template": "protocol_manager_confirm", "example": "pcl protocol-manager --project <project-ref> --confirm-transfer --body-template"}
                ]
            },
            {
                "command": "pcl transfers [--pending|--transfer-id <id>|--reject --body-template]",
                "description": "Inspect and reject protocol manager transfers.",
                "output": "pending transfers, transfer detail, or reject result",
                "actions": [
                    {"name": "pending", "auth": true, "method": "GET", "path": "/views/transfers/pending", "example": "pcl transfers --pending"},
                    {"name": "detail", "auth": true, "method": "GET", "path": "/views/transfers/{transfer_id}", "required_flags": ["--transfer-id"], "example": "pcl transfers --transfer-id <transfer-id>"},
                    {"name": "reject", "auth": true, "method": "POST", "path": "/transfers/reject", "body_template": "transfer_reject", "example": "pcl transfers --reject --body-template"}
                ]
            },
            {
                "command": "pcl events --project <ref> [--audit-log]",
                "description": "Inspect project events and audit logs.",
                "output": "event or audit log data",
                "actions": [
                    {"name": "events", "auth": true, "method": "GET", "path": "/views/projects/{project}/events", "required_flags": ["--project"], "optional_flags": ["--page", "--limit", "--environment"], "example": "pcl events --project <project-ref>"},
                    {"name": "audit_log", "auth": true, "method": "GET", "path": "/views/projects/{project}/audit-log", "required_flags": ["--project"], "optional_flags": ["--page", "--limit", "--environment"], "example": "pcl events --project <project-ref> --audit-log"}
                ]
            },
            {
                "command": "pcl api manifest",
                "description": "Print this agent-readable command manifest.",
            },
            {
                "command": "pcl api list [--filter <term>] [--method <get|post|put|patch|delete>]",
                "description": "List OpenAPI operations with executable inspect and call commands.",
                "output": "operations[] with operation_id, method, path, summary, tags, workflow_alternatives, raw_api_use, inspect_command, call_command",
            },
            {
                "command": "pcl api inspect <operation_id>|<method> <path> [--full]",
                "description": "Inspect a compact operation manifest. Use --full for raw OpenAPI.",
                "output": "operation_id, method, path, auth metadata, workflow_alternatives, raw_api_use, path_params, required_query, body_fields, required_body_fields, body_template, response_statuses, example_call",
            },
            {
                "command": "pcl api call <method> <path[?query]> [--query key=value] [--field key=value] [--body-file body.json] [--paginate <field>] [--page-param page] [--limit-param limit] [--jsonl] [--output <file>] [--dry-run]",
                "description": "Execute any endpoint below /api/v1. Query strings in PATH and repeated --query flags are both accepted; --field merges simple JSON object body fields; GET calls can paginate any array response with --paginate. Add --dry-run to print the request plan without sending it.",
                "output": "request and response status/body; non-2xx responses return structured error envelopes with request_id when the API provides one. Raw calls log operation_id when the live OpenAPI manifest can resolve the method/path.",
                "actions": [
                    {"name": "execute", "method": "*", "path": "<path>", "auth": "default", "optional_flags": ["--dry-run"], "example": "pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated"},
                    {"name": "paginate", "method": "GET", "path": "<path>", "auth": "default", "required_flags": ["--paginate"], "optional_flags": ["--all", "--page", "--limit", "--page-param", "--limit-param", "--max-pages", "--jsonl", "--output"], "example": "pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --output incidents.json"},
                    {"name": "export_jsonl", "method": "GET", "path": "<path>", "auth": "default", "required_flags": ["--paginate", "--jsonl", "--output"], "example": "pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --jsonl --output incidents.jsonl"}
                ]
            },
            {
                "command": "pcl api coverage [--records <n>] [--markdown <path>]",
                "description": "Audit local request history against the live OpenAPI surface. Old records are matched by method/path; new raw api calls also persist operation_id.",
                "output": "total operations, by-method coverage, no-hit operations, hit-but-no-2xx operations, side-effecting no-2xx operations, unmatched records",
            },
        ],
        "examples": [
            "pcl incidents --limit 5",
            "pcl search --query settler",
            "pcl releases list <project-ref>",
            "pcl access members <project-ref>",
            "pcl integrations --project <project-ref> --provider slack",
            "pcl api list --filter incidents",
        ],
    })
}
