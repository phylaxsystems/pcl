use super::{
    super::{
        ApiCommandError,
        SearchArgs,
        WorkflowRequest,
    },
    first_string_field,
    push_query,
    required_arg,
};
use serde_json::Value;

pub(in crate::api) fn search_request(
    args: &SearchArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    if args.health {
        return Ok(WorkflowRequest::get(
            "/health",
            false,
            ["pcl search --system-status"],
        ));
    }
    if args.system_status {
        return Ok(WorkflowRequest::get(
            "/system-status",
            false,
            ["pcl search --stats"],
        ));
    }
    if args.stats {
        return Ok(WorkflowRequest::get(
            "/stats",
            false,
            ["pcl projects list --limit 10"],
        ));
    }
    if args.whitelist {
        return Ok(WorkflowRequest::get(
            "/whitelist",
            true,
            ["pcl projects mine"],
        ));
    }
    if args.verified_contract {
        let address = required_arg(args.address.as_deref(), "--address")?;
        let chain_id = args.chain_id.ok_or_else(|| {
            ApiCommandError::InvalidWorkflowWithActions {
                message: "--verified-contract requires --chain-id".to_string(),
                next_actions: vec![
                    "pcl search --verified-contract --address <address> --chain-id <chain-id>"
                        .to_string(),
                    "pcl search --help".to_string(),
                ],
            }
        })?;
        let mut request = WorkflowRequest::get(
            "/web/verified-contract",
            false,
            ["pcl contracts --project <project-ref>"],
        );
        push_query(&mut request.query, "address", Some(address));
        push_query(&mut request.query, "chainId", Some(chain_id));
        return Ok(request);
    }

    let query = args
        .query
        .as_deref()
        .or(args.term.as_deref())
        .filter(|query| !query.trim().is_empty())
        .ok_or_else(|| {
            ApiCommandError::InvalidWorkflowWithActions {
                message: "Search query is required unless you choose a specific search action"
                    .to_string(),
                next_actions: vec![
                    "pcl search <term>".to_string(),
                    "pcl search --query <term>".to_string(),
                    "pcl search --stats".to_string(),
                    "pcl search --help".to_string(),
                ],
            }
        })?;

    let mut request = WorkflowRequest::get(
        "/search",
        false,
        [
            "pcl projects show <project-ref>",
            "pcl contracts --project <project-ref>",
        ],
    );
    push_query(&mut request.query, "query", Some(query));
    Ok(request)
}

pub(in crate::api) fn search_next_actions(data: &Value, fallback: Vec<String>) -> Vec<String> {
    if let Some(project_id) = data
        .get("projects")
        .and_then(Value::as_array)
        .and_then(|projects| projects.first())
        .and_then(|project| first_string_field(project, &["project_id", "projectId", "id", "slug"]))
    {
        return vec![
            format!("pcl projects show {project_id}"),
            format!("pcl contracts --project {project_id}"),
        ];
    }
    if let Some(project_id) = data
        .get("contracts")
        .and_then(Value::as_array)
        .and_then(|contracts| contracts.first())
        .and_then(|contract| {
            contract.get("data").map_or_else(
                || first_string_field(contract, &["related_project_id", "related_project_slug"]),
                |inner| first_string_field(inner, &["related_project_id", "related_project_slug"]),
            )
        })
    {
        return vec![
            format!("pcl projects show {project_id}"),
            format!("pcl contracts --project {project_id}"),
        ];
    }
    fallback
}

workflow_definition!(
    "search",
    command: "pcl search [--query <term>] [--stats] [--system-status] [--verified-contract --address <addr> --chain-id <id>]",
    description: "Search projects/contracts and inspect platform metadata.",
    output: "search results, stats, system status, health, whitelist, or verified contract data",
    policy: MachineRaw,
    legacy_examples: [

    ],
    actions: [
        action!("query", false, "GET", "/search", "pcl search --query settler", optional: ["--query"]),
        action!("stats", false, "GET", "/stats", "pcl search --stats"),
        action!(
            "system_status",
            false,
            "GET",
            "/system-status",
            "pcl search --system-status"
        ),
        action!("health", false, "GET", "/health", "pcl search --health"),
        action!(
            "whitelist",
            true,
            "GET",
            "/whitelist",
            "pcl search --whitelist"
        ),
        action!("verified_contract", false, "GET", "/web/verified-contract", "pcl search --verified-contract --address 0x... --chain-id 1", required: ["--address", "--chain-id"]),
    ],
);
