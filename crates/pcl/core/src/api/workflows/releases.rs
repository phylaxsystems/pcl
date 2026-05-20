use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        ReleasesArgs,
        WorkflowRequest,
    },
    body_or_empty,
    first_string_field,
    push_query,
    request_body,
    required_arg,
    required_project_arg,
    workflow_with_body,
};
use serde_json::Value;

pub(in crate::api) fn releases_request(
    args: &ReleasesArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "releases", "--project")?;
    if args.preview {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/releases/preview"),
            true,
            body,
            vec![format!(
                "pcl releases create {project} --body-file release.json"
            )],
        ));
    }
    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/releases"),
            true,
            body,
            vec![format!("pcl releases list {project}")],
        ));
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
            return Ok(WorkflowRequest::get(
                format!("/projects/{project}/releases/{release_id}/backtest-progress"),
                true,
                vec![format!("pcl releases show {project} {release_id}")],
            ));
        }
        if args.retry_check {
            let check_id = required_arg(args.check_id.as_deref(), "--check-id")?;
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/checks/{check_id}/retry"),
                true,
                Some(body_or_empty(body)),
                vec![format!(
                    "pcl releases backtest-progress {project} {release_id}"
                )],
            ));
        }
        if args.deploy {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/deploy"),
                true,
                body,
                vec![format!("pcl releases show {project} {release_id}")],
            ));
        }
        if args.remove {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/remove"),
                true,
                body,
                vec![format!("pcl releases list {project}")],
            ));
        }
        if args.deploy_calldata {
            let signer_address = required_arg(args.signer_address.as_deref(), "--signer-address")?;
            let mut request = WorkflowRequest::get(
                format!("/projects/{project}/releases/{release_id}/deploy-calldata"),
                true,
                vec![format!("pcl releases deploy {project} {release_id}")],
            );
            push_query(&mut request.query, "signerAddress", Some(signer_address));
            return Ok(request);
        }
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/releases/{release_id}/remove-calldata"),
            true,
            vec![format!("pcl releases remove {project} {release_id}")],
        ));
    }
    let Some(release_id) = &args.release_id else {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/releases"),
            true,
            vec![format!("pcl releases show {project} <release-id>")],
        ));
    };
    Ok(WorkflowRequest::get(
        format!("/projects/{project}/releases/{release_id}"),
        true,
        vec![
            format!(
                "pcl releases calldata deploy {project} {release_id} --signer-address <signer-address>"
            ),
            format!("pcl releases calldata remove {project} {release_id}"),
        ],
    ))
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
    legacy_examples: [
        "pcl releases --project <project-ref>",
        "pcl releases --project <project-ref> --release-id <release-id>",
        "pcl releases --project <project-ref> --preview --body-file release.json",
    ],
    actions: [
        action!("list", true, "GET", "/projects/{project}/releases", "pcl releases list <project-ref>", required: ["<project-ref>"]),
        action!("detail", true, "GET", "/projects/{project}/releases/{release_id}", "pcl releases show <project-ref> <release-id>", required: ["<project-ref>", "<release-id>"]),
        action!("preview", true, "POST", "/projects/{project}/releases/preview", "pcl releases preview <project-ref> --body-file release.json", required: ["<project-ref>"], body_template: "release"),
        action!("create", true, "POST", "/projects/{project}/releases", "pcl releases create <project-ref> --body-file release.json", required: ["<project-ref>"], body_template: "release"),
        action!("backtest_progress", true, "GET", "/projects/{project}/releases/{release_id}/backtest-progress", "pcl releases backtest-progress <project-ref> <release-id>", required: ["<project-ref>", "<release-id>"]),
        action!("retry_check", true, "POST", "/projects/{project}/releases/{release_id}/checks/{check_id}/retry", "pcl releases retry-check <project-ref> <release-id> <check-id>", required: ["<project-ref>", "<release-id>", "<check-id>"], body_template: "empty_object"),
        action!("deploy_calldata", true, "GET", "/projects/{project}/releases/{release_id}/deploy-calldata", "pcl releases calldata deploy <project-ref> <release-id> --signer-address 0x...", required: ["<project-ref>", "<release-id>", "--signer-address"], query: {"signerAddress" => "<signer-address>"}),
        action!("deploy", true, "POST", "/projects/{project}/releases/{release_id}/deploy", "pcl releases deploy <project-ref> <release-id> --body-template", required: ["<project-ref>", "<release-id>"], body_template: "release_deploy"),
        action!("remove_calldata", true, "GET", "/projects/{project}/releases/{release_id}/remove-calldata", "pcl releases calldata remove <project-ref> <release-id>", required: ["<project-ref>", "<release-id>"]),
        action!("remove", true, "POST", "/projects/{project}/releases/{release_id}/remove", "pcl releases remove <project-ref> <release-id> --body-template", required: ["<project-ref>", "<release-id>"], body_template: "release_remove"),
    ],
);
