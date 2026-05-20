use super::{
    super::{
        AccountArgs,
        ApiCommandError,
        HttpMethod,
        WorkflowRequest,
        definitions::{
            WorkflowActionDefinition,
            WorkflowDefinition,
            WorkflowOutputPolicy,
        },
    },
    body_or_empty,
    request_body,
    workflow_with_body,
};

pub(in crate::api) fn account_request(
    args: &AccountArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.accept_terms {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/web/auth/accept-terms",
            true,
            Some(body_or_empty(body)),
            ["pcl account", "pcl projects mine"],
        ));
    }
    if args.logout {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/web/auth/logout",
            true,
            Some(body_or_empty(body)),
            ["pcl auth logout"],
        ));
    }
    Ok(WorkflowRequest::get(
        "/web/auth/me",
        true,
        ["pcl account --accept-terms", "pcl projects mine"],
    ))
}

pub(in crate::api) const DEFINITION: WorkflowDefinition = WorkflowDefinition {
    name: "account",
    command: "pcl account [--me|--accept-terms|--logout]",
    description: "Inspect authenticated web user state and perform onboarding actions.",
    output: "current user account state, terms acceptance result, or logout result",
    output_policy: WorkflowOutputPolicy::MachineRaw,
    legacy_examples: &[],
    actions: &[
        action!("me", true, "GET", "/web/auth/me", "pcl account"),
        action!("accept_terms", true, "POST", "/web/auth/accept-terms", "pcl account --accept-terms", body_template: "empty_object"),
        action!("logout", true, "POST", "/web/auth/logout", "pcl account --logout", body_template: "empty_object"),
    ],
};
