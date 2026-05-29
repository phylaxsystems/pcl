use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        IntegrationProvider,
        IntegrationsArgs,
        WorkflowOperation,
        WorkflowRequest,
    },
    body_or_empty,
    request_body,
    required_project_arg,
    workflow_operation_get,
    workflow_operation_with_body,
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
    let provider_path = provider.path();
    if args.configure {
        return workflow_operation_with_body(
            integration_operation(provider, IntegrationAction::Configure)
                .path_param("project_id", &project),
            true,
            body,
            vec![format!(
                "pcl integrations --project {project} --provider {provider_path}"
            )],
        );
    }
    if args.test {
        return workflow_operation_with_body(
            integration_operation(provider, IntegrationAction::Test)
                .path_param("project_id", &project),
            true,
            Some(body_or_empty(body)),
            vec![format!(
                "pcl integrations --project {project} --provider {provider_path}"
            )],
        );
    }
    if args.delete {
        return workflow_operation_with_body(
            integration_operation(provider, IntegrationAction::Delete)
                .path_param("project_id", &project),
            true,
            body,
            vec![format!(
                "pcl integrations --project {project} --provider {provider_path}"
            )],
        );
    }
    workflow_operation_get(
        integration_operation(provider, IntegrationAction::Get).path_param("project_id", &project),
        true,
        vec![
            format!("pcl integrations --project {project} --provider {provider_path} --test"),
            format!(
                "pcl integrations --project {project} --provider {provider_path} --configure --body-template"
            ),
        ],
    )
}

#[derive(Clone, Copy)]
enum IntegrationAction {
    Get,
    Configure,
    Test,
    Delete,
}

fn integration_operation(
    provider: IntegrationProvider,
    action: IntegrationAction,
) -> WorkflowOperation {
    match (provider, action) {
        (IntegrationProvider::Slack, IntegrationAction::Get) => {
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_projects_project_id_integrations_slack",
            )
        }
        (IntegrationProvider::Slack, IntegrationAction::Configure) => {
            WorkflowOperation::new(
                HttpMethod::Post,
                "post_projects_project_id_integrations_slack",
            )
        }
        (IntegrationProvider::Slack, IntegrationAction::Test) => {
            WorkflowOperation::new(
                HttpMethod::Post,
                "post_projects_project_id_integrations_slack_test",
            )
        }
        (IntegrationProvider::Slack, IntegrationAction::Delete) => {
            WorkflowOperation::new(
                HttpMethod::Delete,
                "delete_projects_project_id_integrations_slack",
            )
        }
        (IntegrationProvider::Pagerduty, IntegrationAction::Get) => {
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_projects_project_id_integrations_pagerduty",
            )
        }
        (IntegrationProvider::Pagerduty, IntegrationAction::Configure) => {
            WorkflowOperation::new(
                HttpMethod::Post,
                "post_projects_project_id_integrations_pagerduty",
            )
        }
        (IntegrationProvider::Pagerduty, IntegrationAction::Test) => {
            WorkflowOperation::new(
                HttpMethod::Post,
                "post_projects_project_id_integrations_pagerduty_test",
            )
        }
        (IntegrationProvider::Pagerduty, IntegrationAction::Delete) => {
            WorkflowOperation::new(
                HttpMethod::Delete,
                "delete_projects_project_id_integrations_pagerduty",
            )
        }
    }
}

workflow_definition!(
    "integrations",
    command: "pcl integrations --project <ref> --provider <slack|pagerduty> [--configure|--test|--delete]",
    description: "Manage Slack and PagerDuty integrations.",
    output: "integration status or mutation/test results",
    policy: MachineRaw,
    legacy_examples: [

    ],
    actions: [
        action!("get", true, "GET", "/projects/{project}/integrations/{provider}", "pcl integrations --project <project-ref> --provider slack", required: ["--project", "--provider"]),
        action!("configure", true, "POST", "/projects/{project}/integrations/{provider}", "pcl integrations --project <project-ref> --provider slack --configure --body-template", required: ["--project", "--provider"], body_template: "slack|pagerduty"),
        action!("test", true, "POST", "/projects/{project}/integrations/{provider}/test", "pcl integrations --project <project-ref> --provider slack --test", required: ["--project", "--provider"], body_template: "slack|pagerduty"),
        action!("delete", true, "DELETE", "/projects/{project}/integrations/{provider}", "pcl integrations --project <project-ref> --provider slack --delete", required: ["--project", "--provider"]),
    ],
);
