use super::{
    super::{
        AccessArgs,
        ApiCommandError,
        HttpMethod,
        WorkflowOperation,
        WorkflowRequest,
    },
    body_or_empty,
    request_body,
    required_arg,
    required_project_arg,
    workflow_operation_get,
    workflow_operation_with_body,
};

pub(in crate::api) fn access_request(
    args: &AccessArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.pending {
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_invitations_pending"),
            true,
            ["pcl access accept <token>"],
        );
    }
    if args.accept || args.preview {
        let token = required_arg(args.token.as_deref(), "--token")?;
        if args.accept {
            return workflow_operation_with_body(
                WorkflowOperation::new(HttpMethod::Post, "post_invitations_token_accept")
                    .path_param("token", &token),
                true,
                Some(body_or_empty(body)),
                ["pcl projects mine"],
            );
        }
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_invitations_token_preview")
                .path_param("token", &token),
            false,
            vec![format!("pcl access accept {token}")],
        );
    }
    if let Some(token) = &args.token {
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_invitations_token_preview")
                .path_param("token", token),
            false,
            vec![format!("pcl access accept {token}")],
        );
    }
    let project = required_project_arg(args.project.as_deref(), "access", "--project")?;
    if args.my_role {
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_projects_project_id_my_role")
                .path_param("project_id", &project),
            true,
            vec![format!("pcl access members {project}")],
        );
    }
    if args.invite {
        return workflow_operation_with_body(
            WorkflowOperation::new(HttpMethod::Post, "post_projects_project_id_invitations")
                .path_param("project_id", &project),
            true,
            body,
            vec![format!("pcl access invitations {project}")],
        );
    }
    if args.resend || args.revoke {
        let invitation_id = required_arg(args.invitation_id.as_deref(), "--invitation-id")?;
        if args.resend {
            return workflow_operation_with_body(
                WorkflowOperation::new(
                    HttpMethod::Post,
                    "post_projects_project_id_invitations_invitation_id_resend",
                )
                .path_param("project_id", &project)
                .path_param("invitation_id", &invitation_id),
                true,
                Some(body_or_empty(body)),
                vec![format!("pcl access invitations {project}")],
            );
        }
        return workflow_operation_with_body(
            WorkflowOperation::new(
                HttpMethod::Delete,
                "delete_projects_project_id_invitations_invitation_id",
            )
            .path_param("project_id", &project)
            .path_param("invitation_id", &invitation_id),
            true,
            body,
            vec![format!("pcl access invitations {project}")],
        );
    }
    if args.update_role || args.remove {
        let member_user_id = required_arg(args.member_user_id.as_deref(), "--member-user-id")?;
        if args.update_role {
            return workflow_operation_with_body(
                WorkflowOperation::new(
                    HttpMethod::Patch,
                    "patch_projects_project_id_members_member_user_id",
                )
                .path_param("project_id", &project)
                .path_param("member_user_id", &member_user_id),
                true,
                body,
                vec![format!("pcl access members {project}")],
            );
        }
        return workflow_operation_with_body(
            WorkflowOperation::new(
                HttpMethod::Delete,
                "delete_projects_project_id_members_member_user_id",
            )
            .path_param("project_id", &project)
            .path_param("member_user_id", &member_user_id),
            true,
            body,
            vec![format!("pcl access members {project}")],
        );
    }
    if args.invitations {
        return workflow_operation_get(
            WorkflowOperation::new(HttpMethod::Get, "get_projects_project_id_invitations")
                .path_param("project_id", &project),
            true,
            vec![format!("pcl access invite {project} --body-template")],
        );
    }
    workflow_operation_get(
        WorkflowOperation::new(HttpMethod::Get, "get_projects_project_id_members")
            .path_param("project_id", &project),
        true,
        vec![
            format!("pcl access my-role {project}"),
            format!("pcl access invitations {project}"),
        ],
    )
}

workflow_definition!(
    "access",
    command: "pcl access <members|invitations|pending|preview|accept|invite|resend|revoke|role|member|my-role>",
    description: "Manage project members, roles, and invitations.",
    output: "member lists, invitation lists, role data, or mutation results",
    policy: MachineRaw,
    actions: [
        action!("members", true, "get_projects_project_id_members", "pcl access members <project-ref>", required: ["<project-ref>"]),
        action!("my_role", true, "get_projects_project_id_my_role", "pcl access my-role <project-ref>", required: ["<project-ref>"]),
        action!("invitations", true, "get_projects_project_id_invitations", "pcl access invitations <project-ref>", required: ["<project-ref>"]),
        action!("invite", true, "post_projects_project_id_invitations", "pcl access invite <project-ref> --body-template", required: ["<project-ref>"], body_template: "access_invite"),
        action!("resend", true, "post_projects_project_id_invitations_invitation_id_resend", "pcl access resend <project-ref> <invitation-id>", required: ["<project-ref>", "<invitation-id>"], body_template: "empty_object"),
        action!("revoke", true, "delete_projects_project_id_invitations_invitation_id", "pcl access revoke <project-ref> <invitation-id>", required: ["<project-ref>", "<invitation-id>"], body_template: "empty_object"),
        action!("update_role", true, "patch_projects_project_id_members_member_user_id", "pcl access role update <project-ref> <member-user-id> --body-template", required: ["<project-ref>", "<member-user-id>"], body_template: "role_update"),
        action!("remove", true, "delete_projects_project_id_members_member_user_id", "pcl access member remove <project-ref> <member-user-id>", required: ["<project-ref>", "<member-user-id>"], body_template: "empty_object"),
        action!("pending", true, "get_invitations_pending",
            "pcl access pending"
        ),
        action!("preview", false, "get_invitations_token_preview", "pcl access preview <token>", required: ["<token>"]),
        action!("accept", true, "post_invitations_token_accept", "pcl access accept <token>", required: ["<token>"], body_template: "empty_object"),
    ],
);
