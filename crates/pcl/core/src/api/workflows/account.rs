use super::{
    super::{
        AccountArgs,
        ApiCommandError,
        HttpMethod,
        WorkflowOperation,
        WorkflowRequest,
    },
    body_or_empty,
    request_body,
    workflow_operation_get,
    workflow_operation_with_body,
};

pub(in crate::api) fn account_request(
    args: &AccountArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.accept_terms {
        return workflow_operation_with_body(
            WorkflowOperation::new(HttpMethod::Post, "post_web_auth_accept_terms"),
            true,
            Some(body_or_empty(body)),
            ["pcl account", "pcl projects mine"],
        );
    }
    if args.logout {
        return workflow_operation_with_body(
            WorkflowOperation::new(HttpMethod::Post, "post_web_auth_logout"),
            true,
            Some(body_or_empty(body)),
            ["pcl auth logout"],
        );
    }
    workflow_operation_get(
        WorkflowOperation::new(HttpMethod::Get, "get_web_auth_me"),
        true,
        ["pcl account --accept-terms", "pcl projects mine"],
    )
}

workflow_definition!(
    "account",
    command: "pcl account [--me|--accept-terms|--logout]",
    description: "Inspect authenticated web user state and perform onboarding actions.",
    output: "current user account state, terms acceptance result, or logout result",
    policy: MachineRaw,
    legacy_examples: [

    ],
    actions: [
        action!("me", true, "GET", "/web/auth/me", "pcl account"),
        action!("accept_terms", true, "POST", "/web/auth/accept-terms", "pcl account --accept-terms", body_template: "empty_object"),
        action!("logout", true, "POST", "/web/auth/logout", "pcl account --logout", body_template: "empty_object"),
    ],
);
