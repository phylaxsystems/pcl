use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        ReleasesArgs,
        WorkflowOperation,
        WorkflowRequest,
    },
    body_or_empty,
    first_string_field,
    push_query,
    request_body,
    required_arg,
    required_project_arg,
    workflow_operation_with_body,
};
use serde_json::Value;

pub(in crate::api) fn releases_request(
    args: &ReleasesArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "releases", "--project")?;
    if args.preview {
        return workflow_operation_with_body(
            WorkflowOperation::new(
                HttpMethod::Post,
                "post_projects_project_id_releases_preview",
            )
            .path_param("project_id", &project),
            true,
            body,
            vec![format!(
                "pcl releases create {project} --body-file release.json"
            )],
        );
    }
    if args.create {
        return workflow_operation_with_body(
            WorkflowOperation::new(HttpMethod::Post, "post_projects_project_id_releases")
                .path_param("project_id", &project),
            true,
            body,
            vec![format!("pcl releases list {project}")],
        );
    }
    if args.deploy
        || args.remove
        || args.deploy_calldata
        || args.remove_calldata
        || args.backtest_progress
        || args.retry_check
    {
        let release_id = required_arg(args.release_id.as_deref(), "--release-id")?;
        if args.backtest_progress {
            return WorkflowRequest::from_operation(
                WorkflowOperation::new(
                    HttpMethod::Get,
                    "get_projects_project_id_releases_release_id_backtest_progress",
                )
                .path_param("project_id", &project)
                .path_param("release_id", &release_id),
                Vec::new(),
                None,
                true,
                vec![format!("pcl releases show {project} {release_id}")],
            );
        }
        if args.retry_check {
            let check_id = required_arg(args.check_id.as_deref(), "--check-id")?;
            return workflow_operation_with_body(
                WorkflowOperation::new(
                    HttpMethod::Post,
                    "post_projects_project_id_releases_release_id_checks_check_id_retry",
                )
                .path_param("project_id", &project)
                .path_param("release_id", &release_id)
                .path_param("check_id", &check_id),
                true,
                Some(body_or_empty(body)),
                vec![format!(
                    "pcl releases backtest-progress {project} {release_id}"
                )],
            );
        }
        if args.deploy {
            return workflow_operation_with_body(
                WorkflowOperation::new(
                    HttpMethod::Post,
                    "post_projects_project_id_releases_release_id_deploy",
                )
                .path_param("project_id", &project)
                .path_param("release_id", &release_id),
                true,
                body,
                vec![format!("pcl releases show {project} {release_id}")],
            );
        }
        if args.remove {
            return workflow_operation_with_body(
                WorkflowOperation::new(
                    HttpMethod::Post,
                    "post_projects_project_id_releases_release_id_remove",
                )
                .path_param("project_id", &project)
                .path_param("release_id", &release_id),
                true,
                body,
                vec![format!("pcl releases list {project}")],
            );
        }
        if args.deploy_calldata {
            let signer_address = required_arg(args.signer_address.as_deref(), "--signer-address")?;
            let mut request = WorkflowRequest::from_operation(
                WorkflowOperation::new(
                    HttpMethod::Get,
                    "get_projects_project_id_releases_release_id_deploy_calldata",
                )
                .path_param("project_id", &project)
                .path_param("release_id", &release_id),
                Vec::new(),
                None,
                true,
                vec![format!("pcl releases deploy {project} {release_id}")],
            )?;
            push_query(&mut request.query, "signerAddress", Some(signer_address));
            return Ok(request);
        }
        return WorkflowRequest::from_operation(
            WorkflowOperation::new(
                HttpMethod::Get,
                "get_projects_project_id_releases_release_id_remove_calldata",
            )
            .path_param("project_id", &project)
            .path_param("release_id", &release_id),
            Vec::new(),
            None,
            true,
            vec![format!("pcl releases remove {project} {release_id}")],
        );
    }
    let Some(release_id) = &args.release_id else {
        return WorkflowRequest::from_operation(
            WorkflowOperation::new(HttpMethod::Get, "get_projects_project_id_releases")
                .path_param("project_id", &project),
            Vec::new(),
            None,
            true,
            vec![format!("pcl releases show {project} <release-id>")],
        );
    };
    WorkflowRequest::from_operation(
        WorkflowOperation::new(
            HttpMethod::Get,
            "get_projects_project_id_releases_release_id",
        )
        .path_param("project_id", &project)
        .path_param("release_id", release_id),
        Vec::new(),
        None,
        true,
        vec![
            format!(
                "pcl releases calldata deploy {project} {release_id} --signer-address <signer-address>"
            ),
            format!("pcl releases calldata remove {project} {release_id}"),
        ],
    )
}

pub(in crate::api) fn releases_next_actions(
    data: &Value,
    args: &ReleasesArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    let Some(project) = args.project.as_deref() else {
        return fallback;
    };
    if args.release_id.is_some()
        || args.preview
        || args.create
        || args.deploy
        || args.remove
        || args.deploy_calldata
        || args.remove_calldata
        || args.backtest_progress
        || args.retry_check
    {
        return fallback;
    }

    let release_id = data
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| first_string_field(item, &["id", "release_id", "releaseId"]))
        .or_else(|| first_string_field(data, &["id", "release_id", "releaseId"]));

    release_id.map_or(fallback, |release_id| {
        vec![format!("pcl releases show {project} {release_id}")]
    })
}

workflow_definition!(
    "releases",
    command: "pcl releases <list|show|create|preview|deploy|remove|calldata|backtest-progress|retry-check>",
    description: "List, inspect, create, preview, deploy, check progress, retry failed checks, and remove releases.",
    output: "release data, diffs, check progress, deployment confirmations, or calldata",
    policy: MachineRaw,
    actions: [
        action!("list", true, "get_projects_project_id_releases", "pcl releases list <project-ref>", required: ["<project-ref>"]),
        action!("detail", true, "get_projects_project_id_releases_release_id", "pcl releases show <project-ref> <release-id>", required: ["<project-ref>", "<release-id>"]),
        action!("preview", true, "post_projects_project_id_releases_preview", "pcl releases preview <project-ref> --body-file release.json", required: ["<project-ref>"], body_template: "release"),
        action!("create", true, "post_projects_project_id_releases", "pcl releases create <project-ref> --body-file release.json", required: ["<project-ref>"], body_template: "release"),
        action!("backtest_progress", true, "get_projects_project_id_releases_release_id_backtest_progress", "pcl releases backtest-progress <project-ref> <release-id>", required: ["<project-ref>", "<release-id>"]),
        action!("retry_check", true, "post_projects_project_id_releases_release_id_checks_check_id_retry", "pcl releases retry-check <project-ref> <release-id> <check-id>", required: ["<project-ref>", "<release-id>", "<check-id>"], body_template: "empty_object"),
        action!("deploy_calldata", true, "get_projects_project_id_releases_release_id_deploy_calldata", "pcl releases calldata deploy <project-ref> <release-id> --signer-address 0x...", required: ["<project-ref>", "<release-id>", "--signer-address"], query: {"signerAddress" => "<signer-address>"}),
        action!("deploy", true, "post_projects_project_id_releases_release_id_deploy", "pcl releases deploy <project-ref> <release-id> --body-template", required: ["<project-ref>", "<release-id>"], body_template: "release_deploy"),
        action!("remove_calldata", true, "get_projects_project_id_releases_release_id_remove_calldata", "pcl releases calldata remove <project-ref> <release-id>", required: ["<project-ref>", "<release-id>"]),
        action!("remove", true, "post_projects_project_id_releases_release_id_remove", "pcl releases remove <project-ref> <release-id> --body-template", required: ["<project-ref>", "<release-id>"], body_template: "release_remove"),
    ],
);
