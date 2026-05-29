use super::{
    super::{
        AccessArgs,
        ApiCommandError,
        HttpMethod,
        WorkflowRequest,
    },
    body_or_empty,
    request_body,
    required_arg,
    required_project_arg,
    workflow_with_body,
};

pub(in crate::api) fn access_request(
    args: &AccessArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.pending {
        return Ok(WorkflowRequest::get(
            "/invitations/pending",
            true,
            ["pcl access accept <token>"],
        ));
    }
    if args.accept || args.preview {
        let token = required_arg(args.token.as_deref(), "--token")?;
        if args.accept {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/invitations/{token}/accept"),
                true,
                Some(body_or_empty(body)),
                ["pcl projects mine"],
            ));
        }
        return Ok(WorkflowRequest::get(
            format!("/invitations/{token}/preview"),
            false,
            vec![format!("pcl access accept {token}")],
        ));
    }
    if let Some(token) = &args.token {
        return Ok(WorkflowRequest::get(
            format!("/invitations/{token}/preview"),
            false,
            vec![format!("pcl access accept {token}")],
        ));
    }
    let project = required_project_arg(args.project.as_deref(), "access", "--project")?;
    if args.my_role {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/my-role"),
            true,
            vec![format!("pcl access members {project}")],
        ));
    }
    if args.invite {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/invitations"),
            true,
            body,
            vec![format!("pcl access invitations {project}")],
        ));
    }
    if args.resend || args.revoke {
        let invitation_id = required_arg(args.invitation_id.as_deref(), "--invitation-id")?;
        if args.resend {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/invitations/{invitation_id}/resend"),
                true,
                Some(body_or_empty(body)),
                vec![format!("pcl access invitations {project}")],
            ));
        }
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/invitations/{invitation_id}"),
            true,
            body,
            vec![format!("pcl access invitations {project}")],
        ));
    }
    if args.update_role || args.remove {
        let member_user_id = required_arg(args.member_user_id.as_deref(), "--member-user-id")?;
        if args.update_role {
            return Ok(workflow_with_body(
                HttpMethod::Patch,
                format!("/projects/{project}/members/{member_user_id}"),
                true,
                body,
                vec![format!("pcl access members {project}")],
            ));
        }
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/members/{member_user_id}"),
            true,
            body,
            vec![format!("pcl access members {project}")],
        ));
    }
    if args.invitations {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/invitations"),
            true,
            vec![format!("pcl access invite {project} --body-template")],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("/projects/{project}/members"),
        true,
        vec![
            format!("pcl access my-role {project}"),
            format!("pcl access invitations {project}"),
        ],
    ))
}

workflow_definition!(
    "access",
    command: "pcl access <members|invitations|pending|preview|accept|invite|resend|revoke|role|member|my-role>",
    description: "Manage project members, roles, and invitations.",
    output: "member lists, invitation lists, role data, or mutation results",
    policy: MachineRaw,
    legacy_examples: [
        "pcl access --project <project-ref> --members",
        "pcl access --project <project-ref> --invite --body-template",
        "pcl access --token <token> --preview",
    ],
    actions: [
        action!("members", true, "GET", "/projects/{project}/members", "pcl access members <project-ref>", required: ["<project-ref>"]),
        action!("my_role", true, "GET", "/projects/{project}/my-role", "pcl access my-role <project-ref>", required: ["<project-ref>"]),
        action!("invitations", true, "GET", "/projects/{project}/invitations", "pcl access invitations <project-ref>", required: ["<project-ref>"]),
        action!("invite", true, "POST", "/projects/{project}/invitations", "pcl access invite <project-ref> --body-template", required: ["<project-ref>"], body_template: "access_invite"),
        action!("resend", true, "POST", "/projects/{project}/invitations/{invitation_id}/resend", "pcl access resend <project-ref> <invitation-id>", required: ["<project-ref>", "<invitation-id>"], body_template: "empty_object"),
        action!("revoke", true, "DELETE", "/projects/{project}/invitations/{invitation_id}", "pcl access revoke <project-ref> <invitation-id>", required: ["<project-ref>", "<invitation-id>"], body_template: "empty_object"),
        action!("update_role", true, "PATCH", "/projects/{project}/members/{member_user_id}", "pcl access role update <project-ref> <member-user-id> --body-template", required: ["<project-ref>", "<member-user-id>"], body_template: "role_update"),
        action!("remove", true, "DELETE", "/projects/{project}/members/{member_user_id}", "pcl access member remove <project-ref> <member-user-id>", required: ["<project-ref>", "<member-user-id>"], body_template: "empty_object"),
        action!(
            "pending",
            true,
            "GET",
            "/invitations/pending",
            "pcl access pending"
        ),
        action!("preview", false, "GET", "/invitations/{token}/preview", "pcl access preview <token>", required: ["<token>"]),
        action!("accept", true, "POST", "/invitations/{token}/accept", "pcl access accept <token>", required: ["<token>"], body_template: "empty_object"),
    ],
);
