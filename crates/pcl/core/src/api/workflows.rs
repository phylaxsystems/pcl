use super::{
    AccessArgs,
    AccountArgs,
    ApiCommandError,
    AssertionsArgs,
    ContractsArgs,
    DeploymentsArgs,
    EventsArgs,
    HttpMethod,
    IncidentsArgs,
    IntegrationsArgs,
    ProjectsArgs,
    ProtocolManagerArgs,
    ReleasesArgs,
    SearchArgs,
    TransfersArgs,
    WorkflowRequest,
    read_body,
};
use serde_json::{
    Map,
    Value,
    json,
};
use std::path::PathBuf;

pub(super) fn search_request(args: &SearchArgs) -> Result<WorkflowRequest, ApiCommandError> {
    if args.health {
        return Ok(WorkflowRequest::get(
            "/health",
            false,
            ["pcl search --system-status"],
        ));
    }
    if args.system_status {
        return Ok(WorkflowRequest::get(
            "/system-status",
            false,
            ["pcl search --stats"],
        ));
    }
    if args.stats {
        return Ok(WorkflowRequest::get(
            "/stats",
            false,
            ["pcl projects --limit 10"],
        ));
    }
    if args.whitelist {
        return Ok(WorkflowRequest::get(
            "/whitelist",
            true,
            ["pcl projects --mine"],
        ));
    }
    if args.verified_contract {
        let address = required_arg(args.address.as_deref(), "--address")?;
        let chain_id = args.chain_id.ok_or_else(|| {
            ApiCommandError::InvalidWorkflowWithActions {
                message: "--verified-contract requires --chain-id".to_string(),
                next_actions: vec![
                    "pcl search --verified-contract --address <address> --chain-id <chain-id>"
                        .to_string(),
                    "pcl search --help".to_string(),
                ],
            }
        })?;
        let mut request = WorkflowRequest::get(
            "/web/verified-contract",
            false,
            ["pcl contracts --project <project-ref>"],
        );
        push_query(&mut request.query, "address", Some(address));
        push_query(&mut request.query, "chainId", Some(chain_id));
        return Ok(request);
    }

    let query = args
        .query
        .as_deref()
        .or(args.term.as_deref())
        .filter(|query| !query.trim().is_empty())
        .ok_or_else(|| {
            ApiCommandError::InvalidWorkflowWithActions {
                message: "Search query is required unless you choose a specific search action"
                    .to_string(),
                next_actions: vec![
                    "pcl search <term>".to_string(),
                    "pcl search --query <term>".to_string(),
                    "pcl search --stats".to_string(),
                    "pcl search --help".to_string(),
                ],
            }
        })?;

    let mut request = WorkflowRequest::get(
        "/search",
        false,
        [
            "pcl projects --project <project-ref>",
            "pcl contracts --project <project-ref>",
        ],
    );
    push_query(&mut request.query, "query", Some(query));
    Ok(request)
}

pub(super) fn account_request(args: &AccountArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.accept_terms {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/web/auth/accept-terms",
            true,
            Some(body_or_empty(body)),
            ["pcl account", "pcl projects --mine"],
        ));
    }
    if args.logout {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/web/auth/logout",
            true,
            Some(body_or_empty(body)),
            ["pcl auth logout"],
        ));
    }
    Ok(WorkflowRequest::get(
        "/web/auth/me",
        true,
        ["pcl account --accept-terms", "pcl projects --mine"],
    ))
}

pub(super) fn contracts_request(args: &ContractsArgs) -> Result<WorkflowRequest, ApiCommandError> {
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
            ["pcl releases --project <project-ref>"],
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

pub(super) fn releases_request(args: &ReleasesArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "releases", "--project")?;
    if args.preview {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/releases/preview"),
            true,
            body,
            vec![format!(
                "pcl releases --project {project} --create --body-file release.json"
            )],
        ));
    }
    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/releases"),
            true,
            body,
            vec![format!("pcl releases --project {project}")],
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
                vec![format!(
                    "pcl releases --project {project} --release-id {release_id}"
                )],
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
                    "pcl releases --project {project} --release-id {release_id} --backtest-progress"
                )],
            ));
        }
        if args.deploy {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/deploy"),
                true,
                body,
                vec![format!(
                    "pcl releases --project {project} --release-id {release_id}"
                )],
            ));
        }
        if args.remove {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/remove"),
                true,
                body,
                vec![format!("pcl releases --project {project}")],
            ));
        }
        if args.deploy_calldata {
            let signer_address = required_arg(args.signer_address.as_deref(), "--signer-address")?;
            let mut request = WorkflowRequest::get(
                format!("/projects/{project}/releases/{release_id}/deploy-calldata"),
                true,
                vec![format!(
                    "pcl releases --project {project} --release-id {release_id} --deploy"
                )],
            );
            push_query(&mut request.query, "signerAddress", Some(signer_address));
            return Ok(request);
        }
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/releases/{release_id}/remove-calldata"),
            true,
            vec![format!(
                "pcl releases --project {project} --release-id {release_id} --remove"
            )],
        ));
    }
    let Some(release_id) = &args.release_id else {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/releases"),
            true,
            vec![format!(
                "pcl releases --project {project} --release-id <release-id>"
            )],
        ));
    };
    Ok(WorkflowRequest::get(
        format!("/projects/{project}/releases/{release_id}"),
        true,
        vec![
            format!(
                "pcl releases --project {project} --release-id {release_id} --deploy-calldata --signer-address <signer-address>"
            ),
            format!("pcl releases --project {project} --release-id {release_id} --remove-calldata"),
        ],
    ))
}

pub(super) fn deployments_request(
    args: &DeploymentsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "deployments", "--project")?;
    if args.confirm {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/confirm-deployment"),
            true,
            body,
            vec![format!("pcl deployments --project {project}")],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("/views/projects/{project}/deployments"),
        true,
        vec![format!("pcl releases --project {project}")],
    ))
}

pub(super) fn access_request(args: &AccessArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.pending {
        return Ok(WorkflowRequest::get(
            "/invitations/pending",
            true,
            ["pcl access --token <token> --accept"],
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
                ["pcl projects --mine"],
            ));
        }
        return Ok(WorkflowRequest::get(
            format!("/invitations/{token}/preview"),
            false,
            vec![format!("pcl access --token {token} --accept")],
        ));
    }
    if let Some(token) = &args.token {
        return Ok(WorkflowRequest::get(
            format!("/invitations/{token}/preview"),
            false,
            vec![format!("pcl access --token {token} --accept")],
        ));
    }
    let project = required_project_arg(args.project.as_deref(), "access", "--project")?;
    if args.my_role {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/my-role"),
            true,
            vec![format!("pcl access --project {project} --members")],
        ));
    }
    if args.invite {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/invitations"),
            true,
            body,
            vec![format!("pcl access --project {project} --invitations")],
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
                vec![format!("pcl access --project {project} --invitations")],
            ));
        }
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/invitations/{invitation_id}"),
            true,
            body,
            vec![format!("pcl access --project {project} --invitations")],
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
                vec![format!("pcl access --project {project} --members")],
            ));
        }
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/members/{member_user_id}"),
            true,
            body,
            vec![format!("pcl access --project {project} --members")],
        ));
    }
    if args.invitations {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/invitations"),
            true,
            vec![format!(
                "pcl access --project {project} --invite --body-template"
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("/projects/{project}/members"),
        true,
        vec![
            format!("pcl access --project {project} --my-role"),
            format!("pcl access --project {project} --invitations"),
        ],
    ))
}

pub(super) fn integrations_request(
    args: &IntegrationsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "integrations", "--project")?;
    let Some(provider) = args.provider else {
        return Err(ApiCommandError::InvalidWorkflowWithActions {
            message: "--provider is required".to_string(),
            next_actions: vec![
                "pcl integrations --project <project-id> --provider slack".to_string(),
                "pcl integrations --project <project-id> --provider pagerduty".to_string(),
                "pcl integrations --help".to_string(),
            ],
        });
    };
    let provider = provider.path();
    let base = format!("/projects/{project}/integrations/{provider}");
    if args.configure {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            base,
            true,
            body,
            vec![format!(
                "pcl integrations --project {project} --provider {provider}"
            )],
        ));
    }
    if args.test {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("{base}/test"),
            true,
            Some(body_or_empty(body)),
            vec![format!(
                "pcl integrations --project {project} --provider {provider}"
            )],
        ));
    }
    if args.delete {
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            base,
            true,
            body,
            vec![format!(
                "pcl integrations --project {project} --provider {provider}"
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        base,
        true,
        vec![
            format!("pcl integrations --project {project} --provider {provider} --test"),
            format!(
                "pcl integrations --project {project} --provider {provider} --configure --body-template"
            ),
        ],
    ))
}

pub(super) fn protocol_manager_request(
    args: &ProtocolManagerArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "protocol-manager", "--project")?;
    let base = format!("/projects/{project}/protocol-manager");
    if args.nonce {
        let address = required_arg(args.address.as_deref(), "--address")?;
        let mut request = WorkflowRequest::get(
            format!("{base}/nonce"),
            true,
            vec![format!(
                "pcl protocol-manager --project {project} --set --body-template"
            )],
        );
        push_query(&mut request.query, "address", Some(address));
        push_query(&mut request.query, "chain_id", args.chain_id);
        return Ok(request);
    }
    if args.set {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            base,
            true,
            body,
            vec![format!(
                "pcl protocol-manager --project {project} --pending-transfer"
            )],
        ));
    }
    if args.clear {
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            base,
            true,
            body,
            vec![format!(
                "pcl protocol-manager --project {project} --nonce --address <manager-address>"
            )],
        ));
    }
    if args.transfer_calldata {
        let new_manager = required_arg(args.new_manager.as_deref(), "--new-manager")?;
        let mut request = WorkflowRequest::get(
            format!("{base}/transfer-calldata"),
            true,
            vec![format!(
                "pcl protocol-manager --project {project} --set --body-template"
            )],
        );
        push_query(&mut request.query, "new_manager", Some(new_manager));
        return Ok(request);
    }
    if args.accept_calldata {
        return Ok(WorkflowRequest::get(
            format!("{base}/accept-calldata"),
            true,
            vec![format!(
                "pcl protocol-manager --project {project} --confirm-transfer --body-template"
            )],
        ));
    }
    if args.confirm_transfer {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("{base}/confirm-transfer"),
            true,
            body,
            vec![format!(
                "pcl protocol-manager --project {project} --pending-transfer"
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("{base}/pending-transfer"),
        true,
        vec![
            format!("pcl protocol-manager --project {project} --nonce --address <manager-address>"),
            format!(
                "pcl protocol-manager --project {project} --transfer-calldata --new-manager <manager-address>"
            ),
        ],
    ))
}

pub(super) fn transfers_request(args: &TransfersArgs) -> Result<WorkflowRequest, ApiCommandError> {
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

pub(super) fn events_request(args: &EventsArgs) -> Result<WorkflowRequest, ApiCommandError> {
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

fn workflow_with_body(
    method: HttpMethod,
    path: impl Into<String>,
    require_auth: bool,
    body: Option<String>,
    next_actions: impl IntoIterator<Item = impl Into<String>>,
) -> WorkflowRequest {
    WorkflowRequest {
        method,
        path: path.into(),
        query: Vec::new(),
        body,
        require_auth,
        next_actions: next_actions.into_iter().map(Into::into).collect(),
    }
}

fn body_or_empty(body: Option<String>) -> String {
    body.unwrap_or_else(|| "{}".to_string())
}

pub(super) fn request_body(
    body: Option<&str>,
    body_file: Option<&PathBuf>,
    fields: &[String],
) -> Result<Option<String>, ApiCommandError> {
    let body = read_body(body, body_file)?;
    body_with_fields(body, fields)
}

fn project_request_body(args: &ProjectsArgs) -> Result<Option<String>, ApiCommandError> {
    let body = read_body(args.body.as_deref(), args.body_file.as_ref())?;
    let mut object = match body {
        Some(body) => serde_json::from_str::<Value>(&body)?,
        None => Value::Object(Map::new()),
    };
    let Value::Object(map) = &mut object else {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "project body must be a JSON object".to_string(),
        });
    };

    insert_optional(
        map,
        "project_name",
        args.project_name.clone().map(Value::String),
    );
    insert_optional(
        map,
        "project_description",
        args.project_description.clone().map(Value::String),
    );
    insert_optional(
        map,
        "profile_image_url",
        args.profile_image_url.clone().map(Value::String),
    );
    insert_optional(
        map,
        "github_url",
        args.github_url.clone().map(Value::String),
    );
    insert_optional(map, "chain_id", args.chain_id.map(|value| json!(value)));
    insert_optional(map, "is_private", args.is_private.map(|value| json!(value)));
    insert_optional(map, "is_dev", args.is_dev.map(|value| json!(value)));
    apply_fields(map, &args.field)?;

    if map.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::Object(map.clone()).to_string()))
    }
}

fn body_with_fields(
    body: Option<String>,
    fields: &[String],
) -> Result<Option<String>, ApiCommandError> {
    if fields.is_empty() {
        return Ok(body);
    }
    let mut value = match body {
        Some(body) => serde_json::from_str::<Value>(&body)?,
        None => Value::Object(Map::new()),
    };
    let Value::Object(map) = &mut value else {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--field requires the request body to be a JSON object".to_string(),
        });
    };
    apply_fields(map, fields)?;
    Ok(Some(Value::Object(map.clone()).to_string()))
}

fn apply_fields(map: &mut Map<String, Value>, fields: &[String]) -> Result<(), ApiCommandError> {
    for field in fields {
        let (key, value) = field.split_once('=').ok_or_else(|| {
            ApiCommandError::InvalidKeyValue {
                kind: "field",
                input: field.clone(),
            }
        })?;
        map.insert(key.to_string(), parse_field_value(value));
    }
    Ok(())
}

fn parse_field_value(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}

fn insert_optional(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_string(), value);
    }
}

fn required_arg(value: Option<&str>, name: &str) -> Result<String, ApiCommandError> {
    value.map(ToString::to_string).ok_or_else(|| {
        ApiCommandError::InvalidWorkflow {
            message: format!("{name} is required"),
        }
    })
}

fn required_arg_with_actions(
    value: Option<&str>,
    name: &str,
    next_actions: Vec<String>,
) -> Result<String, ApiCommandError> {
    value.map(ToString::to_string).ok_or_else(|| {
        ApiCommandError::InvalidWorkflowWithActions {
            message: format!("{name} is required"),
            next_actions,
        }
    })
}

fn required_project_arg(
    value: Option<&str>,
    command: &str,
    flag: &str,
) -> Result<String, ApiCommandError> {
    required_arg_with_actions(
        value,
        flag,
        vec![
            "pcl projects --mine".to_string(),
            format!("pcl {command} {flag} <project-id>"),
            format!("pcl {command} --help"),
        ],
    )
}

pub(super) fn project_segment(path: &str) -> Option<(&'static str, &str, &str)> {
    if let Some(rest) = path.strip_prefix("/projects/") {
        let (segment, suffix) = split_first_segment(rest);
        if matches!(segment, "saved" | "resolve") {
            return None;
        }
        return Some(("/projects/", segment, suffix));
    }
    if let Some(rest) = path.strip_prefix("/views/projects/") {
        let (segment, suffix) = split_first_segment(rest);
        if segment == "home" {
            return None;
        }
        return Some(("/views/projects/", segment, suffix));
    }
    None
}

fn split_first_segment(path: &str) -> (&str, &str) {
    path.split_once('/').map_or((path, ""), |(segment, _rest)| {
        (segment, &path[segment.len()..])
    })
}

pub(super) fn incidents_request(args: &IncidentsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    if args.all && (args.incident_id.is_some() || args.stats || args.retry_trace) {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--all is only supported for incident list workflows".to_string(),
        });
    }
    if args.stats && args.project_id.is_none() {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--stats requires --project-id".to_string(),
        });
    }
    if args.tx_id.is_some() && args.incident_id.is_none() {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--tx-id requires --incident-id".to_string(),
        });
    }
    if args.retry_trace && args.tx_id.is_none() {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--retry-trace requires --incident-id and --tx-id".to_string(),
        });
    }

    let mut query = Vec::new();
    push_query(&mut query, "page", args.page);
    push_query(&mut query, "limit", args.limit);

    if let Some(incident_id) = &args.incident_id {
        if args.retry_trace {
            let tx_id = required_arg(args.tx_id.as_deref(), "--tx-id")?;
            return Ok(WorkflowRequest {
                method: HttpMethod::Post,
                path: format!("/incidents/{incident_id}/transactions/{tx_id}/trace/retry"),
                query,
                body: Some("{}".to_string()),
                require_auth: true,
                next_actions: vec![format!(
                    "pcl incidents --incident-id {incident_id} --tx-id {tx_id}"
                )],
            });
        }
        let path = if let Some(tx_id) = &args.tx_id {
            format!("/views/incidents/{incident_id}/transactions/{tx_id}/trace")
        } else {
            format!("/views/incidents/{incident_id}")
        };
        let next_actions = vec![
            "pcl incidents --limit 5".to_string(),
            format!("pcl api inspect get {}", path),
        ];
        return Ok(WorkflowRequest::get_with_query(
            path,
            query,
            true,
            next_actions,
        ));
    }

    if let Some(project_id) = &args.project_id {
        if args.stats {
            let path = format!("/projects/{project_id}/incidents/stats");
            return Ok(WorkflowRequest::get_with_query(
                path,
                query,
                true,
                vec![format!(
                    "pcl incidents --project-id {project_id} --limit 10"
                )],
            ));
        }
        push_query(&mut query, "assertionId", args.assertion_id.as_deref());
        push_query(
            &mut query,
            "assertionAdopterId",
            args.assertion_adopter_id.as_deref(),
        );
        push_query(&mut query, "environment", args.environment.as_deref());
        push_query(&mut query, "fromDate", args.from_date.as_deref());
        push_query(&mut query, "toDate", args.to_date.as_deref());
        let path = format!("/views/projects/{project_id}/incidents");
        return Ok(WorkflowRequest::get_with_query(
            path,
            query,
            true,
            vec![
                format!("pcl assertions --project-id {project_id}"),
                "pcl incidents --limit 5".to_string(),
            ],
        ));
    }

    push_query(&mut query, "network", args.network);
    push_query(&mut query, "sort", args.sort.as_deref());
    push_query(&mut query, "devMode", args.dev_mode.as_deref());
    Ok(WorkflowRequest::get_with_query(
        "/views/public/incidents",
        query,
        false,
        vec![
            "pcl incidents --project-id <project-id> --limit 10".to_string(),
            "pcl projects --limit 10".to_string(),
        ],
    ))
}

pub(super) fn incidents_next_actions(
    data: &Value,
    args: &IncidentsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if let Some(incident_id) = &args.incident_id {
        if args.tx_id.is_none()
            && let Some(tx_id) = data
                .get("data")
                .and_then(|data| data.get("invalidating_transactions"))
                .and_then(Value::as_array)
                .and_then(|transactions| transactions.first())
                .and_then(|transaction| {
                    first_string_field(transaction, &["transaction_hash", "id", "tx_id"])
                })
        {
            return vec![
                format!("pcl incidents --incident-id {incident_id} --tx-id {tx_id}"),
                "pcl incidents --limit 5".to_string(),
            ];
        }
        return fallback;
    }
    first_string_field(data, &["id", "incidentId", "incident_id"]).map_or(fallback, |incident_id| {
        vec![
            format!("pcl incidents --incident-id {incident_id}"),
            "pcl projects --limit 10".to_string(),
        ]
    })
}

pub(super) fn projects_next_actions(data: &Value, fallback: Vec<String>) -> Vec<String> {
    if let Some(project_id) = data.get("project_id").and_then(Value::as_str) {
        return vec![
            format!("pcl assertions --project-id {project_id}"),
            format!("pcl incidents --project-id {project_id} --limit 10"),
        ];
    }
    first_string_field(data, &["project_id", "projectId", "id"]).map_or(fallback, |project_id| {
        vec![
            format!("pcl projects --project-id {project_id}"),
            format!("pcl assertions --project-id {project_id}"),
            format!("pcl incidents --project-id {project_id} --limit 10"),
        ]
    })
}

pub(super) fn assertions_next_actions(
    data: &Value,
    args: &AssertionsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    let Some(project_id) = &args.project_id else {
        return first_string_field(
            data,
            &["assertion_adopter_address", "adopter_address", "address"],
        )
        .map_or(fallback, |address| {
            vec![format!("pcl assertions --adopter-address {address}")]
        });
    };

    first_string_field(data, &["assertion_id", "assertionId", "id"]).map_or(
        fallback,
        |assertion_id| {
            vec![
                format!("pcl assertions --project-id {project_id} --assertion-id {assertion_id}",),
                format!("pcl incidents --project-id {project_id} --assertion-id {assertion_id}",),
            ]
        },
    )
}

pub(super) fn search_next_actions(data: &Value, fallback: Vec<String>) -> Vec<String> {
    if let Some(project_id) = data
        .get("projects")
        .and_then(Value::as_array)
        .and_then(|projects| projects.first())
        .and_then(|project| first_string_field(project, &["project_id", "projectId", "id", "slug"]))
    {
        return vec![
            format!("pcl projects --project-id {project_id}"),
            format!("pcl contracts --project {project_id}"),
        ];
    }
    if let Some(project_id) = data
        .get("contracts")
        .and_then(Value::as_array)
        .and_then(|contracts| contracts.first())
        .and_then(|contract| {
            contract.get("data").map_or_else(
                || first_string_field(contract, &["related_project_id", "related_project_slug"]),
                |inner| first_string_field(inner, &["related_project_id", "related_project_slug"]),
            )
        })
    {
        return vec![
            format!("pcl projects --project-id {project_id}"),
            format!("pcl contracts --project {project_id}"),
        ];
    }
    fallback
}

pub(super) fn first_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(value) = object.get(*key).and_then(Value::as_str) {
                    return Some(value.to_string());
                }
            }
            object
                .values()
                .find_map(|value| first_string_field(value, keys))
        }
        Value::Array(values) => {
            values
                .iter()
                .find_map(|value| first_string_field(value, keys))
        }
        _ => None,
    }
}

pub(super) fn projects_request(args: &ProjectsArgs) -> Result<WorkflowRequest, ApiCommandError> {
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
            vec!["pcl projects --mine".to_string()],
        ));
    }

    if args.mine {
        return Ok(WorkflowRequest::get_with_query(
            "/views/projects/home",
            query,
            true,
            vec![
                "pcl account".to_string(),
                "pcl projects --saved --user-id <user-id>".to_string(),
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
            vec!["pcl projects --mine".to_string()],
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
                vec![format!("pcl projects --project-id {project_id}")],
            ));
        }
        if args.widget {
            return Ok(WorkflowRequest::get(
                format!("/projects/{project_id}/widget"),
                true,
                vec![format!("pcl projects --project-id {project_id}")],
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
                    format!("pcl projects --project-id {project_id}"),
                    "pcl projects --mine".to_string(),
                ],
            ));
        }
        if args.update {
            return Ok(workflow_with_body(
                HttpMethod::Put,
                format!("/projects/{project_id}"),
                true,
                body,
                vec![format!("pcl projects --project-id {project_id}")],
            ));
        }
        if args.delete {
            return Ok(workflow_with_body(
                HttpMethod::Delete,
                format!("/projects/{project_id}"),
                true,
                body,
                ["pcl projects --mine"],
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
        [
            "pcl projects --project-id <project-id>",
            "pcl incidents --limit 5",
        ],
    ))
}

pub(super) fn assertions_request(
    args: &AssertionsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    if args.submit || args.submitted {
        return Err(ApiCommandError::InvalidWorkflow {
            message:
                "Submitted assertions have been removed from the API; use releases and registered assertions instead"
                    .to_string(),
        });
    }

    if let Some(adopter_address) = &args.adopter_address {
        let mut request = WorkflowRequest::get(
            "/assertions",
            false,
            ["pcl contracts --project <project-ref>"],
        );
        push_query(&mut request.query, "adopter_address", Some(adopter_address));
        push_query(&mut request.query, "network", args.network.as_deref());
        push_query(
            &mut request.query,
            "environment",
            args.environment.as_deref(),
        );
        push_query(
            &mut request.query,
            "include_onchain_only",
            args.include_onchain_only,
        );
        return Ok(request);
    }

    let project_id =
        required_project_arg(args.project_id.as_deref(), "assertions", "--project-id")?;
    let mut query = Vec::new();
    push_query(&mut query, "page", args.page);
    push_query(&mut query, "limit", args.limit);
    push_query(&mut query, "assertionAdopterId", args.adopter_id.as_deref());
    push_query(&mut query, "environment", args.environment.as_deref());

    if args.registered {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/registered-assertions"),
            true,
            vec![format!("pcl assertions --project-id {project_id}")],
        ));
    }
    if args.remove_info {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/remove-assertions-info"),
            true,
            vec![format!(
                "pcl assertions --project-id {project_id} --remove-calldata"
            )],
        ));
    }
    if args.remove_calldata {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/remove-assertions-calldata"),
            true,
            vec![format!("pcl releases --project {project_id}")],
        ));
    }

    if let Some(assertion_id) = &args.assertion_id {
        return Ok(WorkflowRequest::get_with_query(
            format!("/views/projects/{project_id}/assertions/{assertion_id}"),
            query,
            true,
            vec![format!(
                "pcl incidents --project-id {project_id} --assertion-id {assertion_id}",
            )],
        ));
    }

    Ok(WorkflowRequest::get_with_query(
        format!("/views/projects/{project_id}/assertions"),
        query,
        true,
        vec![
            format!("pcl incidents --project-id {project_id} --limit 10"),
            format!("pcl assertions --project-id {project_id} --assertion-id <assertion-id>"),
        ],
    ))
}

fn push_query<T: ToString>(query: &mut Vec<(String, String)>, name: &str, value: Option<T>) {
    if let Some(value) = value {
        query.push((name.to_string(), value.to_string()));
    }
}
