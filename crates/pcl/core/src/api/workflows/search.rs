use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        SearchArgs,
        WorkflowOperation,
        WorkflowRequest,
    },
    first_string_field,
    push_query,
    required_arg,
    workflow_operation_get,
    workflow_operation_get_with_query,
};
use serde_json::Value;

pub(in crate::api) fn search_request(
    args: &SearchArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    if args.health {
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_health"),
            false,
            ["pcl search --stats"],
        );
    }
    if args.stats {
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_stats"),
            false,
            ["pcl projects list --limit 10"],
        );
    }
    if args.whitelist {
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_whitelist"),
            true,
            ["pcl projects mine"],
        );
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
        let mut query = Vec::new();
        push_query(&mut query, "address", Some(address));
        push_query(&mut query, "chainId", Some(chain_id));
        return workflow_operation_get_with_query(
            WorkflowOperation::new(HttpMethod::Get, "get_web_verified_contract"),
            query,
            false,
            ["pcl contracts --project <project-ref>"],
        );
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

    let mut query_params = Vec::new();
    push_query(&mut query_params, "query", Some(query));
    workflow_operation_get_with_query(
        WorkflowOperation::new(HttpMethod::Get, "get_search"),
        query_params,
        false,
        [
            "pcl projects show <project-ref>",
            "pcl contracts --project <project-ref>",
        ],
    )
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
    command: "pcl search [--query <term>] [--stats] [--health] [--verified-contract --address <addr> --chain-id <id>]",
    description: "Search projects/contracts and inspect platform metadata.",
    output: "search results, stats, health, whitelist, or verified contract data",
    policy: MachineRaw,
    actions: [
        action!("query", false, "get_search", "pcl search --query settler", optional: ["--query"]),
        action!("stats", false, "get_stats", "pcl search --stats"),
        action!("health", false, "get_health", "pcl search --health"),
        action!("whitelist", true, "get_whitelist",
            "pcl search --whitelist"
        ),
        action!("verified_contract", false, "get_web_verified_contract", "pcl search --verified-contract --address 0x... --chain-id 1", required: ["--address", "--chain-id"]),
    ],
);
