use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        TransfersArgs,
        WorkflowRequest,
        definitions::{
            WorkflowActionDefinition,
            WorkflowDefinition,
            WorkflowOutputPolicy,
        },
    },
    first_string_field,
    request_body,
    workflow_with_body,
};
use serde_json::Value;

pub(in crate::api) fn transfers_request(
    args: &TransfersArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.reject {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/transfers/reject",
            true,
            body,
            ["pcl transfers --pending"],
        ));
    }
    if let Some(transfer_id) = &args.transfer_id {
        return Ok(WorkflowRequest::get(
            format!("/views/transfers/{transfer_id}"),
            true,
            ["pcl transfers --pending"],
        ));
    }
    Ok(WorkflowRequest::get(
        "/views/transfers/pending",
        true,
        ["pcl transfers --transfer-id <transfer-id>"],
    ))
}

pub(in crate::api) fn transfers_next_actions(
    data: &Value,
    args: &TransfersArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if args.transfer_id.is_some() || args.reject {
        return fallback;
    }
    first_string_field(data, &["transfer_id", "transferId", "id"]).map_or(fallback, |transfer_id| {
        vec![format!("pcl transfers --transfer-id {transfer_id}")]
    })
}

pub(in crate::api) const DEFINITION: WorkflowDefinition = WorkflowDefinition {
    name: "transfers",
    command: "pcl transfers [--pending|--transfer-id <id>|--reject --body-template]",
    description: "Inspect and reject protocol manager transfers.",
    output: "pending transfers, transfer detail, or reject result",
    output_policy: WorkflowOutputPolicy::MachineRaw,
    legacy_examples: &[],
    actions: &[
        action!(
            "pending",
            true,
            "GET",
            "/views/transfers/pending",
            "pcl transfers --pending"
        ),
        action!("detail", true, "GET", "/views/transfers/{transfer_id}", "pcl transfers --transfer-id <transfer-id>", required: ["--transfer-id"]),
        action!("reject", true, "POST", "/transfers/reject", "pcl transfers --reject --body-template", body_template: "transfer_reject"),
    ],
};
