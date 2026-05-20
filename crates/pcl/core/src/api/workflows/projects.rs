use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        ProjectsArgs,
        WorkflowRequest,
        definitions::{
            WorkflowActionDefinition,
            WorkflowDefinition,
            WorkflowOutputPolicy,
        },
    },
    first_string_field,
    project_request_body,
    push_query,
    required_arg,
    required_project_arg,
    workflow_with_body,
};
use serde_json::{
    Value,
    json,
};

pub(in crate::api) fn projects_next_actions(data: &Value, fallback: Vec<String>) -> Vec<String> {
    if let Some(project_id) = data.get("project_id").and_then(Value::as_str) {
        return vec![
            format!("pcl assertions --project-id {project_id}"),
            format!("pcl incidents --project-id {project_id} --limit 10"),
        ];
    }
    first_string_field(data, &["project_id", "projectId", "id"]).map_or(fallback, |project_id| {
        vec![
            format!("pcl projects show {project_id}"),
            format!("pcl assertions --project-id {project_id}"),
            format!("pcl incidents --project-id {project_id} --limit 10"),
        ]
    })
}

pub(in crate::api) fn projects_request(
    args: &ProjectsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let mut query = Vec::new();
    push_query(&mut query, "page", args.page);
    push_query(&mut query, "limit", args.limit);
    push_query(&mut query, "search", args.search.as_deref());
    let body = project_request_body(args)?;

    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/projects",
            true,
            body,
            vec!["pcl projects mine".to_string()],
        ));
    }

    if args.mine {
        return Ok(WorkflowRequest::get_with_query(
            "/views/projects/home",
            query,
            true,
            vec![
                "pcl account".to_string(),
                "pcl projects saved --user-id <user-id>".to_string(),
            ],
        ));
    }
    if args.saved {
        let user_id = required_arg(args.user_id.as_deref(), "--user-id")?;
        push_query(&mut query, "user_id", Some(user_id));
        return Ok(WorkflowRequest::get_with_query(
            "/projects/saved",
            query,
            true,
            vec!["pcl projects mine".to_string()],
        ));
    }
    if args.project_id.is_none()
        && (args.update || args.delete || args.save || args.unsave || args.resolve || args.widget)
    {
        required_project_arg(args.project_id.as_deref(), "projects", "--project-id")?;
    }
    if let Some(project_id) = &args.project_id {
        if args.resolve {
            return Ok(WorkflowRequest::get_with_query(
                format!("/projects/resolve/{project_id}"),
                query,
                false,
                vec![format!("pcl projects show {project_id}")],
            )
            .with_optional_auth());
        }
        if args.widget {
            return Ok(WorkflowRequest::get(
                format!("/projects/{project_id}/widget"),
                true,
                vec![format!("pcl projects show {project_id}")],
            ));
        }
        if args.save || args.unsave {
            return Ok(workflow_with_body(
                if args.save {
                    HttpMethod::Post
                } else {
                    HttpMethod::Delete
                },
                "/projects/saved",
                true,
                Some(json!({ "project_id": project_id }).to_string()),
                vec![
                    format!("pcl projects show {project_id}"),
                    "pcl projects mine".to_string(),
                ],
            ));
        }
        if args.update {
            return Ok(workflow_with_body(
                HttpMethod::Put,
                format!("/projects/{project_id}"),
                true,
                body,
                vec![format!("pcl projects show {project_id}")],
            ));
        }
        if args.delete {
            return Ok(workflow_with_body(
                HttpMethod::Delete,
                format!("/projects/{project_id}"),
                true,
                body,
                ["pcl projects mine"],
            ));
        }
        return Ok(WorkflowRequest::get_with_query(
            format!("/projects/{project_id}"),
            query,
            true,
            vec![
                format!("pcl assertions --project-id {project_id}"),
                format!("pcl incidents --project-id {project_id} --limit 10"),
            ],
        ));
    }

    Ok(WorkflowRequest::get_with_query(
        "/views/projects",
        query,
        false,
        ["pcl projects show <project-id>", "pcl incidents --limit 5"],
    ))
}

pub(in crate::api) const DEFINITION: WorkflowDefinition = WorkflowDefinition {
    name: "projects",
    command: "pcl projects <list|mine|show|saved|create|update|delete|save|unsave|resolve|widget>",
    description: "List, inspect, create, update, save, unsave, resolve, widget, and delete projects.",
    output: "project explorer, your projects, project detail, saved projects, widget, or mutation result",
    output_policy: WorkflowOutputPolicy::MachineRaw,
    legacy_examples: &[
        "pcl projects --mine",
        "pcl projects --project <project-ref>",
        "pcl projects --create --project-name demo --chain-id 1",
    ],
    actions: &[
        action!(
            "explorer",
            false,
            "GET",
            "/views/projects",
            "pcl projects list --limit 10"
        ),
        action!("mine", true, "GET", "/views/projects/home", "pcl projects mine", aliases: ["pcl projects --mine", "pcl projects --home"]),
        action!("saved", true, "GET", "/projects/saved", "pcl projects saved --user-id <user-id>", required: ["--user-id"], query: {"user_id" => "<user-id>"}),
        action!("detail", true, "GET", "/projects/{project_id}", "pcl projects show <project-ref>", required: ["<project-ref>"]),
        action!("create", true, "POST", "/projects", "pcl projects create --project-name demo --chain-id 1", body_template: "project_create", required_body: ["project_name", "chain_id"]),
        action!("update", true, "PUT", "/projects/{project_id}", "pcl projects update <project-ref> --field github_url=https://github.com/org/repo", required: ["<project-ref>"], body_template: "project_update"),
        action!("delete", true, "DELETE", "/projects/{project_id}", "pcl projects delete <project-ref>", required: ["<project-ref>"]),
        action!("save", true, "POST", "/projects/saved", "pcl projects save <project-ref>", required: ["<project-ref>"], body_template: "project_saved"),
        action!("unsave", true, "DELETE", "/projects/saved", "pcl projects unsave <project-ref>", required: ["<project-ref>"], body_template: "project_saved"),
        action!("resolve", false, "GET", "/projects/resolve/{project_ref}", "pcl projects resolve <project-ref>", required: ["<project-ref>"]),
        action!("widget", true, "GET", "/projects/{project_id}/widget", "pcl projects widget <project-ref>", required: ["<project-ref>"]),
    ],
};
