use super::{
    super::{
        ApiCommandError,
        AssertionsArgs,
        HttpMethod,
        WorkflowOperation,
        WorkflowRequest,
    },
    first_string_field,
    push_query,
    required_project_arg,
    workflow_operation_get,
    workflow_operation_get_with_query,
};
use serde_json::Value;

pub(in crate::api) fn assertions_next_actions(
    data: &Value,
    args: &AssertionsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    let Some(project_id) = &args.project_id else {
        return first_string_field(
            data,
            &["assertion_adopter_address", "adopter_address", "address"],
        )
        .map_or(fallback, |address| {
            vec![format!("pcl assertions --adopter-address {address}")]
        });
    };

    first_string_field(data, &["assertion_id", "assertionId", "id"]).map_or(
        fallback,
        |assertion_id| {
            vec![
                format!("pcl assertions --project-id {project_id} --assertion-id {assertion_id}",),
                format!("pcl incidents --project-id {project_id} --assertion-id {assertion_id}",),
            ]
        },
    )
}

pub(in crate::api) fn assertions_request(
    args: &AssertionsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    if args.submit || args.submitted {
        return Err(ApiCommandError::InvalidWorkflow {
            message:
                "Submitted assertions have been removed from the API; use releases and registered assertions instead"
                    .to_string(),
        });
    }

    if let Some(adopter_address) = &args.adopter_address {
        let mut query = Vec::new();
        push_query(&mut query, "adopter_address", Some(adopter_address));
        push_query(&mut query, "network", args.network.as_deref());
        push_query(&mut query, "environment", args.environment.as_deref());
        push_query(
            &mut query,
            "include_onchain_only",
            args.include_onchain_only,
        );
        return workflow_operation_get_with_query(
            WorkflowOperation::new(HttpMethod::Get, "get_assertions"),
            query,
            false,
            ["pcl contracts --project <project-ref>"],
        );
    }

    let project_id =
        required_project_arg(args.project_id.as_deref(), "assertions", "--project-id")?;
    let mut query = Vec::new();
    push_query(&mut query, "page", args.page);
    push_query(&mut query, "limit", args.limit);
    push_query(&mut query, "assertionAdopterId", args.adopter_id.as_deref());
    push_query(&mut query, "environment", args.environment.as_deref());

    if args.registered {
        return workflow_operation_get(
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_projects_project_id_registered_assertions",
            )
            .path_param("project_id", &project_id),
            true,
            vec![format!("pcl assertions --project-id {project_id}")],
        );
    }
    if args.remove_info {
        return workflow_operation_get(
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_projects_project_id_remove_assertions_info",
            )
            .path_param("project_id", &project_id),
            true,
            vec![format!(
                "pcl assertions --project-id {project_id} --remove-calldata"
            )],
        );
    }
    if args.remove_calldata {
        return workflow_operation_get(
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_projects_project_id_remove_assertions_calldata",
            )
            .path_param("project_id", &project_id),
            true,
            vec![format!("pcl releases list {project_id}")],
        );
    }

    if let Some(assertion_id) = &args.assertion_id {
        return workflow_operation_get_with_query(
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_views_projects_project_id_assertions_assertion_id",
            )
            .path_param("projectId", &project_id)
            .path_param("assertionId", assertion_id),
            query,
            true,
            vec![format!(
                "pcl incidents --project-id {project_id} --assertion-id {assertion_id}",
            )],
        );
    }

    workflow_operation_get_with_query(
        WorkflowOperation::new(HttpMethod::Get, "get_views_projects_project_id_assertions")
            .path_param("projectId", &project_id),
        query,
        true,
        vec![
            format!("pcl incidents --project-id {project_id} --limit 10"),
            format!("pcl assertions --project-id {project_id} --assertion-id <assertion-id>"),
        ],
    )
}

workflow_definition!(
    "assertions",
    command: "pcl assertions --project <ref> [--assertion-id <id>|--registered|--remove-info|--remove-calldata]",
    description: "List, inspect, and manage project assertion lifecycle state.",
    output: "assertion index/detail, registered assertions, or removal info/calldata",
    policy: MachineRaw,
    legacy_examples: [

    ],
    actions: [
        action!("index", true, "GET", "/views/projects/{projectId}/assertions", "pcl assertions --project <project-ref>", required: ["--project"]),
        action!("detail", true, "GET", "/views/projects/{projectId}/assertions/{assertionId}", "pcl assertions --project <project-ref> --assertion-id <assertion-id>", required: ["--project", "--assertion-id"]),
        action!("adopter_lookup", false, "GET", "/assertions", "pcl assertions --adopter-address 0x... --network 1", required: ["--adopter-address"], optional: ["--network", "--environment", "--include-onchain-only"]),
        action!("registered", true, "GET", "/projects/{project_id}/registered-assertions", "pcl assertions --project <project-ref> --registered", required: ["--project"]),
        action!("remove_info", true, "GET", "/projects/{project_id}/remove-assertions-info", "pcl assertions --project <project-ref> --remove-info", required: ["--project"]),
        action!("remove_calldata", true, "GET", "/projects/{project_id}/remove-assertions-calldata", "pcl assertions --project <project-ref> --remove-calldata", required: ["--project"]),
    ],
);
