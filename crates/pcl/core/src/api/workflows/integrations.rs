use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        IntegrationsArgs,
        WorkflowRequest,
        definitions::{
            WorkflowActionDefinition,
            WorkflowDefinition,
            WorkflowOutputPolicy,
        },
    },
    body_or_empty,
    request_body,
    required_project_arg,
    workflow_with_body,
};

pub(in crate::api) fn integrations_request(
    args: &IntegrationsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "integrations", "--project")?;
    let Some(provider) = args.provider else {
        return Err(ApiCommandError::InvalidWorkflowWithActions {
            message: "--provider is required".to_string(),
            next_actions: vec![
                "pcl integrations --project <project-id> --provider slack".to_string(),
                "pcl integrations --project <project-id> --provider pagerduty".to_string(),
                "pcl integrations --help".to_string(),
            ],
        });
    };
    let provider = provider.path();
    let base = format!("/projects/{project}/integrations/{provider}");
    if args.configure {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            base,
            true,
            body,
            vec![format!(
                "pcl integrations --project {project} --provider {provider}"
            )],
        ));
    }
    if args.test {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("{base}/test"),
            true,
            Some(body_or_empty(body)),
            vec![format!(
                "pcl integrations --project {project} --provider {provider}"
            )],
        ));
    }
    if args.delete {
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            base,
            true,
            body,
            vec![format!(
                "pcl integrations --project {project} --provider {provider}"
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        base,
        true,
        vec![
            format!("pcl integrations --project {project} --provider {provider} --test"),
            format!(
                "pcl integrations --project {project} --provider {provider} --configure --body-template"
            ),
        ],
    ))
}

pub(in crate::api) const DEFINITION: WorkflowDefinition = WorkflowDefinition {
    name: "integrations",
    command: "pcl integrations --project <ref> --provider <slack|pagerduty> [--configure|--test|--delete]",
    description: "Manage Slack and PagerDuty integrations.",
    output: "integration status or mutation/test results",
    output_policy: WorkflowOutputPolicy::MachineRaw,
    legacy_examples: &[],
    actions: &[
        action!("get", true, "GET", "/projects/{project}/integrations/{provider}", "pcl integrations --project <project-ref> --provider slack", required: ["--project", "--provider"]),
        action!("configure", true, "POST", "/projects/{project}/integrations/{provider}", "pcl integrations --project <project-ref> --provider slack --configure --body-template", required: ["--project", "--provider"], body_template: "slack|pagerduty"),
        action!("test", true, "POST", "/projects/{project}/integrations/{provider}/test", "pcl integrations --project <project-ref> --provider slack --test", required: ["--project", "--provider"], body_template: "slack|pagerduty"),
        action!("delete", true, "DELETE", "/projects/{project}/integrations/{provider}", "pcl integrations --project <project-ref> --provider slack --delete", required: ["--project", "--provider"]),
    ],
};
