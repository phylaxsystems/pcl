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
    required_arg,
    workflow_operation_get,
    workflow_operation_get_with_query,
};
use serde_json::Value;

pub(in crate::api) fn contracts_request(
    args: &ContractsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
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
                "assertionAdopters.removeAssertionsCalldata",
            )
            .path_param("aa_address", &address),
            query,
            true,
            ["pcl releases list <project-ref>"],
        );
    }
    if let Some(project) = &args.project {
        if let Some(adopter_id) = &args.adopter_id {
            return workflow_operation_get(
                WorkflowOperation::new(HttpMethod::Get, "views.contractDetail.getContractDetail")
                    .path_param("projectId", project)
                    .path_param("adopterId", adopter_id),
                true,
                vec![format!("pcl contracts --project {project}")],
            );
        }
        return workflow_operation_get(
            WorkflowOperation::new(
                HttpMethod::Get,
                "views.projectContracts.getProjectContracts",
            )
            .path_param("projectId", project),
            true,
            vec![format!(
                "pcl contracts --project {project} --adopter-id <adopter-id>"
            )],
        );
    }

    Err(ApiCommandError::InvalidWorkflowWithActions {
        message:
            "Choose a contracts workflow: --project or --remove-calldata; manage project membership through releases"
                .to_string(),
        next_actions: vec![
            "pcl contracts --project <project-ref>".to_string(),
            "pcl releases list <project-ref>".to_string(),
        ],
    })
}

pub(in crate::api) fn contracts_next_actions(
    data: &Value,
    args: &ContractsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if args.project.is_none() || args.adopter_id.is_some() || args.remove_calldata {
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
    command: "pcl contracts [--project <ref>] [--adopter-id <id>] [--remove-calldata --aa-address <address> --assertion-id <id>]",
    description: "Inspect release-based project contracts and generate assertion-removal calldata.",
    output: "release-based contract views or assertion-removal calldata",
    policy: MachineRaw,
    actions: [
        action!("list_project", true, "views.projectContracts.getProjectContracts", "pcl contracts --project <project-ref>", required: ["--project"]),
        action!("detail", true, "views.contractDetail.getContractDetail", "pcl contracts --project <project-ref> --adopter-id <adopter-id>", required: ["--project", "--adopter-id"]),
        action!("remove_calldata", true, "assertionAdopters.removeAssertionsCalldata", "pcl contracts --aa-address 0x... --remove-calldata --network 1 --assertion-id 0x...", required: ["--aa-address", "--assertion-id"], optional: ["--network", "--environment"], query: {"assertion_ids" => "<assertion-id>", "network" => "<chain-id>", "environment" => "production|staging"}),
    ],
);
