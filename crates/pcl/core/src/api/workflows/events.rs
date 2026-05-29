use super::{
    super::{
        ApiCommandError,
        EventsArgs,
        WorkflowRequest,
    },
    push_query,
    required_project_arg,
};

pub(in crate::api) fn events_request(
    args: &EventsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let project = required_project_arg(args.project.as_deref(), "events", "--project")?;
    let mut request = if args.audit_log {
        WorkflowRequest::get(
            format!("/views/projects/{project}/audit-log"),
            true,
            vec![format!("pcl events --project {project}")],
        )
    } else {
        WorkflowRequest::get(
            format!("/views/projects/{project}/events"),
            true,
            vec![format!("pcl events --project {project} --audit-log")],
        )
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
