use super::{
    super::{
        ApiCommandError,
        DeploymentsArgs,
        HttpMethod,
        WorkflowRequest,
    },
    redact_large_artifacts,
    request_body,
    required_project_arg,
    workflow_with_body,
};
use serde_json::Value;

pub(in crate::api) fn deployments_request(
    args: &DeploymentsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "deployments", "--project")?;
    if args.confirm {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/confirm-deployment"),
            true,
            body,
            vec![format!("pcl deployments --project {project}")],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("/views/projects/{project}/deployments"),
        true,
        vec![format!("pcl releases list {project}")],
    ))
}

pub(in crate::api) fn compact_deployment_data(data: &Value) -> Value {
    redact_large_artifacts(data)
}

workflow_definition!(
    "deployments",
    command: "pcl deployments --project <ref> [--confirm --body-template]",
    description: "Inspect deployment state and confirm deployed assertions.",
    output: "deployment view or confirmation result",
    policy: MachineRawHumanCompactArtifacts,
    legacy_examples: [

    ],
    actions: [
        action!("list", true, "GET", "/views/projects/{project}/deployments", "pcl deployments --project <project-ref>", required: ["--project"]),
        action!("confirm", true, "POST", "/projects/{project}/confirm-deployment", "pcl deployments --project <project-ref> --confirm --body-template", required: ["--project"], body_template: "deployment_confirmation"),
    ],
);
