use super::{
    super::{
        ApiCommandError,
        ContractsArgs,
        HttpMethod,
        WorkflowOperation,
        WorkflowRequest,
    },
    first_string_field,
    workflow_operation_get,
};
use serde_json::Value;

pub(in crate::api) fn contracts_request(
    args: &ContractsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let Some(project) = args.project.as_deref() else {
        return Err(ApiCommandError::InvalidWorkflowWithActions {
            message: "Choose a contracts workflow with --project; manage project membership through releases"
                .to_string(),
            next_actions: vec![
                "pcl contracts --project <project-ref>".to_string(),
                "pcl releases list <project-ref>".to_string(),
            ],
        });
    };
    if let Some(adopter_id) = &args.adopter_id {
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "views.contractDetail.getContractDetail")
                .path_param("projectId", project)
                .path_param("adopterId", adopter_id),
            true,
            vec![format!("pcl contracts --project {project}")],
        );
    }
    workflow_operation_get(
        WorkflowOperation::new(
            HttpMethod::Get,
            "views.projectContracts.getProjectContracts",
        )
        .path_param("projectId", project),
        true,
        vec![format!(
            "pcl contracts --project {project} --adopter-id <adopter-id>"
        )],
    )
}

pub(in crate::api) fn contracts_next_actions(
    data: &Value,
    args: &ContractsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if args.adopter_id.is_some() {
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
    command: "pcl contracts --project <ref> [--adopter-id <id>]",
    description: "Inspect release-based project contracts.",
    output: "release-based contract views",
    actions: [
        action!("list_project", true, "views.projectContracts.getProjectContracts", "pcl contracts --project <project-ref>", required: ["--project"]),
        action!("detail", true, "views.contractDetail.getContractDetail", "pcl contracts --project <project-ref> --adopter-id <adopter-id>", required: ["--project", "--adopter-id"]),
    ],
);
