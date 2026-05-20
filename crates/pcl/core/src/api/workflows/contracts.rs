use super::{
    super::{
        ApiCommandError,
        ContractsArgs,
        HttpMethod,
        WorkflowRequest,
        definitions::{
            WorkflowActionDefinition,
            WorkflowDefinition,
            WorkflowOutputPolicy,
        },
    },
    first_string_field,
    push_query,
    request_body,
    required_arg,
    workflow_with_body,
};
use serde_json::Value;

pub(in crate::api) fn contracts_request(
    args: &ContractsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/assertion_adopters",
            true,
            body,
            ["pcl contracts --unassigned --manager <manager-address>"],
        ));
    }
    if args.assign_project {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/assertion_adopters/assign-project",
            true,
            body,
            ["pcl contracts --project <project-ref>"],
        ));
    }
    if args.unassigned {
        let manager = required_arg(args.manager.as_deref(), "--manager")?;
        let mut request = WorkflowRequest::get(
            "/assertion_adopters/no-project",
            true,
            ["pcl contracts --assign-project --body-template"],
        );
        push_query(&mut request.query, "manager", Some(manager));
        return Ok(request);
    }
    if args.remove_calldata {
        let address = required_arg(args.aa_address.as_deref(), "--aa-address")?;
        if args.assertion_ids.is_empty() {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--assertion-id is required for --remove-calldata".to_string(),
            });
        }
        let mut request = WorkflowRequest::get(
            format!("/assertion_adopters/{address}/remove-assertions-calldata"),
            true,
            ["pcl releases list <project-ref>"],
        );
        push_query(&mut request.query, "network", args.network.as_deref());
        push_query(
            &mut request.query,
            "environment",
            args.environment.as_deref(),
        );
        for assertion_id in &args.assertion_ids {
            push_query(&mut request.query, "assertion_ids", Some(assertion_id));
        }
        return Ok(request);
    }
    if args.remove {
        let project = required_arg(args.project.as_deref(), "--project")?;
        let address = required_arg(args.aa_address.as_deref(), "--aa-address")?;
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/{address}"),
            true,
            body,
            vec![format!("pcl contracts --project {project}")],
        ));
    }
    if let Some(project) = &args.project {
        if let Some(adopter_id) = &args.adopter_id {
            return Ok(WorkflowRequest::get(
                format!("/views/projects/{project}/contracts/{adopter_id}"),
                true,
                vec![format!("pcl contracts --project {project}")],
            ));
        }
        return Ok(WorkflowRequest::get(
            format!("/views/projects/{project}/contracts"),
            true,
            vec![format!(
                "pcl contracts --project {project} --adopter-id <adopter-id>"
            )],
        ));
    }

    Ok(WorkflowRequest::get(
        "/assertion_adopters",
        true,
        ["pcl contracts --unassigned --manager <manager-address>"],
    ))
}

pub(in crate::api) fn contracts_next_actions(
    data: &Value,
    args: &ContractsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if args.project.is_none()
        || args.adopter_id.is_some()
        || args.unassigned
        || args.create
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

pub(in crate::api) const DEFINITION: WorkflowDefinition = WorkflowDefinition {
    name: "contracts",
    command: "pcl contracts [--project <ref>] [--adopter-id <id>] [--unassigned --manager <address>] [--create --body-template]",
    description: "List and manage project contracts and assertion adopters.",
    output: "contract views, adopter records, assignment results, or remove calldata",
    output_policy: WorkflowOutputPolicy::MachineRaw,
    legacy_examples: &[],
    actions: &[
        action!(
            "list_all",
            true,
            "GET",
            "/assertion_adopters",
            "pcl contracts"
        ),
        action!("list_project", true, "GET", "/views/projects/{project}/contracts", "pcl contracts --project <project-ref>", required: ["--project"]),
        action!("detail", true, "GET", "/views/projects/{project}/contracts/{adopter_id}", "pcl contracts --project <project-ref> --adopter-id <adopter-id>", required: ["--project", "--adopter-id"]),
        action!("unassigned", true, "GET", "/assertion_adopters/no-project", "pcl contracts --unassigned --manager 0x...", required: ["--manager"], query: {"manager" => "<manager-address>"}),
        action!("create", true, "POST", "/assertion_adopters", "pcl contracts --create --body-template", body_template: "contracts"),
        action!("assign_project", true, "POST", "/assertion_adopters/assign-project", "pcl contracts --assign-project --body-template", body_template: "contracts_assign_project"),
        action!("remove", true, "DELETE", "/projects/{project}/{aa_address}", "pcl contracts --project <project-ref> --aa-address 0x... --remove", required: ["--project", "--aa-address"]),
        action!("remove_calldata", true, "GET", "/assertion_adopters/{aa_address}/remove-assertions-calldata", "pcl contracts --aa-address 0x... --remove-calldata --network 1 --assertion-id 0x...", required: ["--aa-address", "--assertion-id"], optional: ["--network", "--environment"], query: {"assertion_ids" => "<assertion-id>", "network" => "<chain-id>", "environment" => "production|staging"}),
    ],
};
