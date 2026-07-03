use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        ProtocolManagerArgs,
        WorkflowOperation,
        WorkflowRequest,
    },
    first_string_field,
    push_query,
    request_body,
    required_arg,
    required_project_arg,
    workflow_operation_get,
    workflow_operation_with_body,
};
use serde_json::Value;

pub(in crate::api) fn protocol_manager_request(
    args: &ProtocolManagerArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "protocol-manager", "--project")?;
    if args.nonce {
        let address = required_arg(args.address.as_deref(), "--address")?;
        let mut request = workflow_operation_get(
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_projects_project_id_protocol_manager_nonce",
            )
            .path_param("project_id", &project),
            true,
            vec![format!(
                "pcl protocol-manager --project {project} --set --body-template"
            )],
        )?;
        push_query(&mut request.query, "address", Some(address));
        push_query(&mut request.query, "chain_id", args.chain_id);
        return Ok(request);
    }
    if args.set {
        return workflow_operation_with_body(
            WorkflowOperation::new(
                HttpMethod::Post,
                "post_projects_project_id_protocol_manager",
            )
            .path_param("project_id", &project),
            true,
            body,
            vec![format!(
                "pcl protocol-manager --project {project} --pending-transfer"
            )],
        );
    }
    if args.clear {
        return workflow_operation_with_body(
            WorkflowOperation::new(
                HttpMethod::Delete,
                "delete_projects_project_id_protocol_manager",
            )
            .path_param("project_id", &project),
            true,
            body,
            vec![format!(
                "pcl protocol-manager --project {project} --nonce --address <manager-address>"
            )],
        );
    }
    if args.transfer_calldata {
        let new_manager = required_arg(args.new_manager.as_deref(), "--new-manager")?;
        let mut request = workflow_operation_get(
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_projects_project_id_protocol_manager_transfer_calldata",
            )
            .path_param("project_id", &project),
            true,
            vec![format!(
                "pcl protocol-manager --project {project} --set --body-template"
            )],
        )?;
        push_query(&mut request.query, "new_manager", Some(new_manager));
        return Ok(request);
    }
    if args.accept_calldata {
        return workflow_operation_get(
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_projects_project_id_protocol_manager_accept_calldata",
            )
            .path_param("project_id", &project),
            true,
            vec![format!(
                "pcl protocol-manager --project {project} --confirm-transfer --body-template"
            )],
        );
    }
    if args.confirm_transfer {
        return workflow_operation_with_body(
            WorkflowOperation::new(
                HttpMethod::Post,
                "post_projects_project_id_protocol_manager_confirm_transfer",
            )
            .path_param("project_id", &project),
            true,
            body,
            vec![format!(
                "pcl protocol-manager --project {project} --pending-transfer"
            )],
        );
    }
    workflow_operation_get(
        WorkflowOperation::new(
            HttpMethod::Get,
            "get_projects_project_id_protocol_manager_pending_transfer",
        )
        .path_param("project_id", &project),
        true,
        vec![
            format!("pcl protocol-manager --project {project} --nonce --address <manager-address>"),
            format!(
                "pcl protocol-manager --project {project} --transfer-calldata --new-manager <manager-address>"
            ),
        ],
    )
}

pub(in crate::api) fn protocol_manager_next_actions(
    data: &Value,
    args: &ProtocolManagerArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if args.nonce
        || args.set
        || args.clear
        || args.transfer_calldata
        || args.accept_calldata
        || args.confirm_transfer
    {
        return fallback;
    }
    let Some(project) = args.project.as_deref() else {
        return fallback;
    };
    let Some(current_manager) = first_string_field(
        data,
        &[
            "current_manager_address",
            "currentManagerAddress",
            "manager_address",
            "managerAddress",
        ],
    ) else {
        return fallback;
    };

    let mut next_actions = vec![format!(
        "pcl protocol-manager --project {project} --nonce --address {current_manager}"
    )];
    if data
        .get("has_pending_transfer")
        .or_else(|| data.get("hasPendingTransfer"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        next_actions.push(format!(
            "pcl protocol-manager --project {project} --accept-calldata"
        ));
    } else {
        next_actions.push(format!(
            "pcl protocol-manager --project {project} --transfer-calldata --new-manager <manager-address>"
        ));
    }
    next_actions
}

workflow_definition!(
    "protocol-manager",
    command: "pcl protocol-manager --project <ref> [--nonce --address <address>|--set|--clear|--transfer-calldata|--accept-calldata|--pending-transfer|--confirm-transfer]",
    description: "Manage protocol manager transfers and calldata.",
    output: "manager state, nonce, calldata, pending transfer, or mutation result",
    policy: MachineRaw,
    actions: [
        action!("pending_transfer", true, "get_projects_project_id_protocol_manager_pending_transfer", "pcl protocol-manager --project <project-ref> --pending-transfer", required: ["--project"]),
        action!("nonce", true, "get_projects_project_id_protocol_manager_nonce", "pcl protocol-manager --project <project-ref> --nonce --address 0x...", required: ["--project", "--address"], optional: ["--chain-id"], query: {"address" => "<address>", "chain_id" => "<chain-id>"}),
        action!("set", true, "post_projects_project_id_protocol_manager", "pcl protocol-manager --project <project-ref> --set --body-template", required: ["--project"], body_template: "protocol_manager_set"),
        action!("clear", true, "delete_projects_project_id_protocol_manager", "pcl protocol-manager --project <project-ref> --clear", required: ["--project"], body_template: "empty_object"),
        action!("transfer_calldata", true, "get_projects_project_id_protocol_manager_transfer_calldata", "pcl protocol-manager --project <project-ref> --transfer-calldata --new-manager 0x... --broadcast", required: ["--project", "--new-manager"], query: {"new_manager" => "<address>"}),
        action!("accept_calldata", true, "get_projects_project_id_protocol_manager_accept_calldata", "pcl protocol-manager --project <project-ref> --accept-calldata --broadcast", required: ["--project"]),
        action!("confirm_transfer", true, "post_projects_project_id_protocol_manager_confirm_transfer", "pcl protocol-manager --project <project-ref> --confirm-transfer --body-template", required: ["--project"], body_template: "protocol_manager_confirm"),
    ],
);
