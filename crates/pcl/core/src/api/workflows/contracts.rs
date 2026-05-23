use super::{
    super::{
        ApiCommandError,
        ContractsArgs,
        HttpMethod,
        WorkflowOperation,
        WorkflowRequest,
    },
    first_string_field,
    push_query,
    request_body,
    required_arg,
    workflow_operation_get,
    workflow_operation_get_with_query,
    workflow_operation_with_body,
};
use serde_json::Value;

pub(in crate::api) fn contracts_request(
    args: &ContractsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.assign_project {
        return workflow_operation_with_body(
            WorkflowOperation::new(HttpMethod::Post, "post_assertion_adopters_assign_project"),
            true,
            body,
            ["pcl contracts --project <project-ref>"],
        );
    }
    if args.unassigned {
        let manager = required_arg(args.manager.as_deref(), "--manager")?;
        let mut query = Vec::new();
        push_query(&mut query, "manager", Some(manager));
        return workflow_operation_get_with_query(
            WorkflowOperation::new(HttpMethod::Get, "get_assertion_adopters_no_project"),
            query,
            true,
            ["pcl contracts --assign-project --body-template"],
        );
    }
    if args.remove_calldata {
        let address = required_arg(args.aa_address.as_deref(), "--aa-address")?;
        if args.assertion_ids.is_empty() {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--assertion-id is required for --remove-calldata".to_string(),
            });
        }
        let mut query = Vec::new();
        push_query(&mut query, "network", args.network.as_deref());
        push_query(&mut query, "environment", args.environment.as_deref());
        for assertion_id in &args.assertion_ids {
            push_query(&mut query, "assertion_ids", Some(assertion_id));
        }
        return workflow_operation_get_with_query(
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_assertion_adopters_aa_address_remove_assertions_calldata",
            )
            .path_param("aa_address", &address),
            query,
            true,
            ["pcl releases list <project-ref>"],
        );
    }
    if args.remove {
        let project = required_arg(args.project.as_deref(), "--project")?;
        let address = required_arg(args.aa_address.as_deref(), "--aa-address")?;
        return workflow_operation_with_body(
            WorkflowOperation::new(HttpMethod::Delete, "delete_projects_project_id_aa_contract")
                .path_param("project_id", &project)
                .path_param("aa_contract", &address),
            true,
            body,
            vec![format!("pcl contracts --project {project}")],
        );
    }
    if let Some(project) = &args.project {
        if let Some(adopter_id) = &args.adopter_id {
            return workflow_operation_get(
                WorkflowOperation::new(
                    HttpMethod::Get,
                    "get_views_projects_project_id_contracts_adopter_id",
                )
                .path_param("projectId", project)
                .path_param("adopterId", adopter_id),
                true,
                vec![format!("pcl contracts --project {project}")],
            );
        }
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_views_projects_project_id_contracts")
                .path_param("projectId", project),
            true,
            vec![format!(
                "pcl contracts --project {project} --adopter-id <adopter-id>"
            )],
        );
    }

    Err(ApiCommandError::InvalidWorkflowWithActions {
        message:
            "Choose a generated contracts workflow: --project, --unassigned, --assign-project, --remove, or --remove-calldata"
                .to_string(),
        next_actions: vec![
            "pcl contracts --project <project-ref>".to_string(),
            "pcl contracts --unassigned --manager <manager-address>".to_string(),
            "pcl contracts --assign-project --body-template".to_string(),
        ],
    })
}

pub(in crate::api) fn contracts_next_actions(
    data: &Value,
    args: &ContractsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if args.project.is_none()
        || args.adopter_id.is_some()
        || args.unassigned
        || args.assign_project
        || args.remove
        || args.remove_calldata
    {
        return fallback;
    }
    let Some(project) = args.project.as_deref() else {
        return fallback;
    };
    first_string_field(
        data,
        &[
            "assertion_adopter_id",
            "assertionAdopterId",
            "adopter_id",
            "adopterId",
            "id",
        ],
    )
    .map_or(fallback, |adopter_id| {
        vec![format!(
            "pcl contracts --project {project} --adopter-id {adopter_id}"
        )]
    })
}

workflow_definition!(
    "contracts",
    command: "pcl contracts [--project <ref>] [--adopter-id <id>] [--unassigned --manager <address>] [--assign-project --body-template]",
    description: "List and manage project contracts and assertion adopters.",
    output: "contract views, adopter records, assignment results, or remove calldata",
    policy: MachineRaw,
    actions: [
        action!("list_project", true, "get_views_projects_project_id_contracts", "pcl contracts --project <project-ref>", required: ["--project"]),
        action!("detail", true, "get_views_projects_project_id_contracts_adopter_id", "pcl contracts --project <project-ref> --adopter-id <adopter-id>", required: ["--project", "--adopter-id"]),
        action!("unassigned", true, "get_assertion_adopters_no_project", "pcl contracts --unassigned --manager 0x...", required: ["--manager"], query: {"manager" => "<manager-address>"}),
        action!("assign_project", true, "post_assertion_adopters_assign_project", "pcl contracts --assign-project --body-template", body_template: "contracts_assign_project"),
        action!("remove", true, "delete_projects_project_id_aa_contract", "pcl contracts --project <project-ref> --aa-address 0x... --remove", required: ["--project", "--aa-address"]),
        action!("remove_calldata", true, "get_assertion_adopters_aa_address_remove_assertions_calldata", "pcl contracts --aa-address 0x... --remove-calldata --network 1 --assertion-id 0x...", required: ["--aa-address", "--assertion-id"], optional: ["--network", "--environment"], query: {"assertion_ids" => "<assertion-id>", "network" => "<chain-id>", "environment" => "production|staging"}),
    ],
);
