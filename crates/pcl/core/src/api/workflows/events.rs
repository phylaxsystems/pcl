use super::{
    super::{
        ApiCommandError,
        EventsArgs,
        HttpMethod,
        WorkflowOperation,
        WorkflowRequest,
    },
    push_query,
    required_project_arg,
    workflow_operation_get,
};

pub(in crate::api) fn events_request(
    args: &EventsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let project = required_project_arg(args.project.as_deref(), "events", "--project")?;
    let mut request = if args.audit_log {
        workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_views_projects_project_id_audit_log")
                .path_param("projectId", &project),
            true,
            vec![format!("pcl events --project {project}")],
        )?
    } else {
        workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_views_projects_project_id_events")
                .path_param("projectId", &project),
            true,
            vec![format!("pcl events --project {project} --audit-log")],
        )?
    };
    push_query(&mut request.query, "page", args.page);
    push_query(&mut request.query, "limit", args.limit);
    push_query(
        &mut request.query,
        "environment",
        args.environment.as_deref(),
    );
    Ok(request)
}

workflow_definition!(
    "events",
    command: "pcl events --project <ref> [--audit-log]",
    description: "Inspect project events and audit logs.",
    output: "event or audit log data",
    policy: MachineRaw,
    legacy_examples: [

    ],
    actions: [
        action!("events", true, "GET", "/views/projects/{project}/events", "pcl events --project <project-ref>", required: ["--project"], optional: ["--page", "--limit", "--environment"]),
        action!("audit_log", true, "GET", "/views/projects/{project}/audit-log", "pcl events --project <project-ref> --audit-log", required: ["--project"], optional: ["--page", "--limit", "--environment"]),
    ],
);
