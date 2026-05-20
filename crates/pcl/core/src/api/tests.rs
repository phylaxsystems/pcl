use super::*;
use crate::config::{
    CliConfig,
    UserAuth,
};
use chrono::{
    TimeZone,
    Utc,
};
use clap::Parser;
use mockito::Matcher;
use pcl_common::args::{
    CliArgs,
    OutputMode,
};
use serde_json::{
    Value,
    json,
};
use std::{
    cell::Cell,
    fs,
    path::Path,
};

fn test_request_log_path() -> &'static Path {
    Path::new("/tmp/pcl-test-requests.jsonl")
}

fn test_api(api_url: impl AsRef<str>, allow_unauthenticated: bool) -> ApiArgs {
    ApiArgs {
        command: ApiCommand::Manifest,
        api_url: api_url.as_ref().parse().unwrap(),
        allow_unauthenticated,
        refresh_after_401: Cell::new(true),
    }
}

fn valid_auth_config(access_token: &str, refresh_token: &str) -> CliConfig {
    auth_config(access_token, refresh_token, 2030, Some("agent@example.com"))
}

fn expired_auth_config(access_token: &str, refresh_token: &str) -> CliConfig {
    auth_config(access_token, refresh_token, 2020, Some("agent@example.com"))
}

fn auth_config(
    access_token: &str,
    refresh_token: &str,
    expires_year: i32,
    email: Option<&str>,
) -> CliConfig {
    CliConfig {
        auth: Some(UserAuth {
            access_token: access_token.to_string(),
            refresh_token: refresh_token.to_string(),
            expires_at: Utc.with_ymd_and_hms(expires_year, 1, 1, 0, 0, 0).unwrap(),
            refresh_expires_at: None,
            user_id: None,
            wallet_address: None,
            email: email.map(ToString::to_string),
        }),
    }
}

fn assert_output_contains(output: &str, expected: &[&str]) {
    for &needle in expected {
        assert!(
            output.contains(needle),
            "expected output to contain {needle:?}:\n{output}"
        );
    }
}

fn assert_output_omits(output: &str, forbidden: &[&str]) {
    for &needle in forbidden {
        assert!(
            !output.contains(needle),
            "expected output to omit {needle:?}:\n{output}"
        );
    }
}

fn assertions_args(project_id: Option<&str>) -> AssertionsArgs {
    AssertionsArgs {
        project_id: project_id.map(ToString::to_string),
        assertion_id: None,
        adopter_id: None,
        adopter_address: None,
        network: None,
        include_onchain_only: None,
        environment: None,
        page: None,
        limit: None,
        submitted: false,
        registered: false,
        submit: false,
        remove_info: false,
        remove_calldata: false,
        field: Vec::new(),
        body: None,
        body_file: None,
        body_template: false,
    }
}

fn projects_args() -> ProjectsArgs {
    ProjectsArgs {
        project_id: None,
        mine: false,
        saved: false,
        user_id: None,
        page: None,
        limit: None,
        search: None,
        create: false,
        update: false,
        delete: false,
        save: false,
        unsave: false,
        resolve: false,
        widget: false,
        project_name: None,
        project_description: None,
        profile_image_url: None,
        github_url: None,
        chain_id: None,
        is_private: None,
        is_dev: None,
        field: Vec::new(),
        body: None,
        body_file: None,
        body_template: false,
    }
}

fn protocol_manager_args() -> ProtocolManagerArgs {
    ProtocolManagerArgs {
        project: Some("project-1".to_string()),
        nonce: false,
        set: false,
        clear: false,
        transfer_calldata: false,
        accept_calldata: false,
        pending_transfer: false,
        confirm_transfer: false,
        new_manager: None,
        address: None,
        chain_id: None,
        body: None,
        field: Vec::new(),
        body_file: None,
        body_template: false,
    }
}

fn contracts_args() -> ContractsArgs {
    ContractsArgs {
        project: None,
        adopter_id: None,
        aa_address: None,
        manager: None,
        network: None,
        environment: None,
        assertion_ids: Vec::new(),
        unassigned: false,
        create: false,
        assign_project: false,
        remove: false,
        remove_calldata: false,
        body: None,
        field: Vec::new(),
        body_file: None,
        body_template: false,
    }
}

fn access_args() -> AccessArgs {
    AccessArgs {
        project: Some("project-1".to_string()),
        member_user_id: None,
        invitation_id: None,
        token: None,
        members: false,
        invitations: false,
        pending: false,
        preview: false,
        accept: false,
        invite: false,
        resend: false,
        revoke: false,
        update_role: false,
        remove: false,
        my_role: false,
        body: None,
        field: Vec::new(),
        body_file: None,
        body_template: false,
    }
}

fn release_args() -> ReleasesArgs {
    ReleasesArgs {
        project: Some("project-1".to_string()),
        release_id: None,
        signer_address: None,
        check_id: None,
        create: false,
        preview: false,
        deploy: false,
        remove: false,
        deploy_calldata: false,
        remove_calldata: false,
        backtest_progress: false,
        retry_check: false,
        body: None,
        field: Vec::new(),
        body_file: None,
        body_template: false,
    }
}

fn incidents_args() -> IncidentsArgs {
    IncidentsArgs {
        project_id: None,
        incident_id: None,
        tx_id: None,
        assertion_id: None,
        assertion_adopter_id: None,
        environment: None,
        from_date: None,
        to_date: None,
        page: None,
        limit: None,
        network: None,
        sort: None,
        dev_mode: None,
        stats: false,
        retry_trace: false,
        all: false,
        max_pages: None,
        output: None,
        jsonl: false,
    }
}

fn transfers_args() -> TransfersArgs {
    TransfersArgs {
        transfer_id: None,
        pending: false,
        reject: false,
        body: None,
        field: Vec::new(),
        body_file: None,
        body_template: false,
    }
}

#[test]
fn parses_key_values() {
    let parsed = parse_key_values("query", &["limit=5".to_string()]).unwrap();
    assert_eq!(parsed, vec![("limit".to_string(), "5".to_string())]);
}

#[test]
fn parses_inline_query_strings() {
    let (path, query) = split_path_and_inline_query(
        "/projects/project-1/incidents?environment=production&limit=50",
    )
    .unwrap();

    assert_eq!(path, "/projects/project-1/incidents");
    assert_eq!(
        query,
        vec![
            ("environment".to_string(), "production".to_string()),
            ("limit".to_string(), "50".to_string()),
        ]
    );
}

#[test]
fn lists_and_inspects_operations() {
    let spec = json!({
        "paths": {
            "/views/public/incidents": {
                "get": {
                    "operationId": "get_views_public_incidents",
                    "summary": "Get public incidents",
                    "tags": ["views"]
                }
            }
        }
    });

    let operations = list_operations(&spec, Some("incidents"), Some(HttpMethod::Get)).unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].operation_id, "get_views_public_incidents");

    let operation = inspect_operation(&spec, "get_views_public_incidents", None, false).unwrap();
    assert_eq!(operation["method"], "GET");
    assert_eq!(operation["path"], "/views/public/incidents");
}

#[test]
fn openapi_call_commands_include_required_inputs() {
    let spec = json!({
        "paths": {
            "/projects/{project_id}/widgets": {
                "post": {
                    "operationId": "post_project_widgets",
                    "parameters": [
                        {
                            "name": "project_id",
                            "in": "path",
                            "required": true,
                            "schema": {"type": "string"}
                        },
                        {
                            "name": "environment",
                            "in": "query",
                            "required": true,
                            "schema": {"type": "string"}
                        }
                    ],
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "required": ["name"],
                                    "properties": {
                                        "name": {"type": "string"}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let operations = list_operations(&spec, Some("widgets"), Some(HttpMethod::Post)).unwrap();
    let operation = operations.first().unwrap();
    assert!(operation.requires_input);
    assert_eq!(
        operation.input_placeholders,
        vec![
            "path:project_id".to_string(),
            "query:environment".to_string(),
            "body".to_string()
        ]
    );
    assert_eq!(
        operation.call_command,
        "pcl api call post '/projects/<project_id>/widgets' --query 'environment=<environment>' --body '{\"name\":\"<string>\"}'"
    );
    assert_eq!(
        next_actions_for_operations(&operations),
        vec![
            "pcl api inspect post_project_widgets --toon".to_string(),
            "Inspect the operation, then fill the placeholders in the example call".to_string()
        ]
    );

    let inspected = inspect_operation(&spec, "post_project_widgets", None, false).unwrap();
    assert_eq!(inspected["example_call"], operation.call_command);
    assert_eq!(
        inspected["input_placeholders"],
        json!(["path:project_id", "query:environment", "body"])
    );
}

#[test]
fn openapi_next_actions_prefer_runnable_safe_workflow_examples() {
    let spec = json!({
        "paths": {
            "/incidents/{incident_id}": {
                "get": {
                    "operationId": "get_incidents_incident_id",
                    "summary": "Get incident details",
                    "parameters": [
                        {"name": "incident_id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ]
                }
            },
            "/views/public/incidents": {
                "get": {
                    "operationId": "get_views_public_incidents",
                    "summary": "Get public incidents"
                }
            }
        }
    });

    let operations = list_operations(&spec, Some("incidents"), Some(HttpMethod::Get)).unwrap();

    assert_eq!(
        next_actions_for_operations(&operations),
        vec![
            "pcl incidents --limit 5 --toon".to_string(),
            "pcl api inspect get_views_public_incidents --toon".to_string(),
        ]
    );
}

#[test]
fn openapi_next_actions_skip_destructive_workflow_examples() {
    let spec = json!({
        "paths": {
            "/projects/{project_id}/protocol-manager": {
                "delete": {
                    "operationId": "delete_projects_project_id_protocol_manager",
                    "summary": "Clear protocol manager for a project",
                    "tags": ["projects"],
                    "parameters": [
                        {"name": "project_id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ]
                },
                "post": {
                    "operationId": "post_projects_project_id_protocol_manager",
                    "summary": "Set protocol manager for a project",
                    "tags": ["projects"],
                    "parameters": [
                        {"name": "project_id", "in": "path", "required": true, "schema": {"type": "string"}}
                    ],
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": {"type": "object"}
                            }
                        }
                    }
                }
            }
        }
    });

    let operations = list_operations(&spec, Some("protocol-manager"), None).unwrap();

    assert_eq!(
        next_actions_for_operations(&operations),
        vec![
            "pcl protocol-manager --project <project-ref> --set --body-template --toon".to_string(),
            "pcl api inspect post_projects_project_id_protocol_manager --toon".to_string(),
        ]
    );

    let inspected_delete = inspect_operation(
        &spec,
        "delete_projects_project_id_protocol_manager",
        None,
        false,
    )
    .unwrap();
    assert_eq!(
        command_next_actions(&inspected_delete),
        vec!["Review data.workflow_alternatives before running mutating workflow commands"]
    );
}

#[test]
fn openapi_next_actions_keep_safe_remove_calldata_examples() {
    let inspected = json!({
        "method": "GET",
        "workflow_alternatives": [
            {
                "example": "pcl assertions --project-id <project-ref> --remove-calldata"
            }
        ]
    });

    assert_eq!(
        command_next_actions(&inspected),
        vec!["pcl assertions --project-id <project-ref> --remove-calldata"]
    );
}

#[test]
fn public_openapi_call_commands_opt_out_of_local_auth() {
    let health = json!({});
    assert_eq!(
        example_call(HttpMethod::Get, "/health", &health),
        "pcl api call get /health --allow-unauthenticated"
    );
    assert_eq!(
        example_call(HttpMethod::Get, "/openapi", &health),
        "pcl api call get /openapi --allow-unauthenticated"
    );
    assert_eq!(
        example_call(HttpMethod::Get, "/projects", &health),
        "pcl api call get /projects --allow-unauthenticated"
    );
    assert_eq!(
        example_call(HttpMethod::Get, "/cli/auth/code", &health),
        "pcl api call get /cli/auth/code --allow-unauthenticated"
    );
    assert_eq!(
        example_call(HttpMethod::Get, "/cli/auth/status", &health),
        "pcl api call get /cli/auth/status --allow-unauthenticated"
    );
    assert_eq!(
        example_call(HttpMethod::Post, "/projects", &json!({"requestBody": {}})),
        "pcl api call post /projects --body '{}'"
    );
    assert_eq!(
        example_call(
            HttpMethod::Post,
            "/cli/auth/verify",
            &json!({"requestBody": {}})
        ),
        "pcl api call post /cli/auth/verify --allow-unauthenticated --body '{}'"
    );
    assert_eq!(
        example_call(
            HttpMethod::Post,
            "/auth/refresh",
            &json!({"requestBody": {}})
        ),
        "pcl api call post /auth/refresh --allow-unauthenticated --body '{}'"
    );
    assert_eq!(
        example_call(
            HttpMethod::Post,
            "/indexer/assertion-registered",
            &json!({"requestBody": {}})
        ),
        "pcl api call post /indexer/assertion-registered --allow-unauthenticated --header 'x-api-key=<x-api-key>' --body '{}'"
    );
    assert_eq!(
        example_call(
            HttpMethod::Post,
            "/backtesting/events",
            &json!({
                "parameters": [
                    {
                        "name": "check_id",
                        "in": "query",
                        "required": true,
                        "schema": {"type": "string"}
                    }
                ],
                "requestBody": {}
            })
        ),
        "pcl api call post /backtesting/events --allow-unauthenticated --header 'x-api-key=<x-api-key>' --query 'check_id=<check_id>' --body '{}'"
    );

    let public_incidents = json!({
        "parameters": [
            {
                "name": "limit",
                "in": "query",
                "required": true,
                "schema": {"type": "integer"}
            }
        ]
    });
    assert_eq!(
        example_call(
            HttpMethod::Get,
            "/views/public/incidents",
            &public_incidents
        ),
        "pcl api call get /views/public/incidents --allow-unauthenticated --query 'limit=<limit>'"
    );
    assert_eq!(
        example_call(HttpMethod::Get, "/public/incidents", &public_incidents),
        "pcl api call get /public/incidents --allow-unauthenticated --query 'limit=<limit>'"
    );
    assert_eq!(
        example_call(HttpMethod::Get, "/views/incidents/incident-1", &health),
        "pcl api call get /views/incidents/incident-1"
    );
    assert_eq!(
        example_call(HttpMethod::Get, "/incidents/incident-1", &health),
        "pcl api call get /incidents/incident-1"
    );

    let public_with_optional_auth = json!({
        "parameters": [
            {
                "name": "Authorization",
                "in": "header",
                "required": false,
                "schema": {"type": "string"}
            },
            {
                "name": "address",
                "in": "query",
                "required": true,
                "schema": {"type": "string"}
            }
        ]
    });
    assert_eq!(
        example_call(
            HttpMethod::Get,
            "/web/verified-contract",
            &public_with_optional_auth
        ),
        "pcl api call get /web/verified-contract --allow-unauthenticated --query 'address=<address>'"
    );

    let authenticated = json!({
        "parameters": [
            {
                "name": "Authorization",
                "in": "header",
                "required": false,
                "schema": {"type": "string"}
            }
        ]
    });
    assert_eq!(
        example_call(HttpMethod::Get, "/web/auth/me", &authenticated),
        "pcl api call get /web/auth/me"
    );
}

#[test]
fn openapi_call_commands_include_required_headers() {
    let bootstrap = json!({
        "parameters": [
            {
                "name": "authorization",
                "in": "header",
                "required": true,
                "schema": {
                    "type": "string",
                    "pattern": "^Bearer .+$"
                }
            }
        ],
        "requestBody": {}
    });

    assert_eq!(
        example_call(HttpMethod::Post, "/web/auth/bootstrap-session", &bootstrap),
        "pcl api call post /web/auth/bootstrap-session --header 'authorization=Bearer <privy-token>' --body '{}'"
    );
    assert_eq!(
        operation_input_placeholders("/web/auth/bootstrap-session", &bootstrap),
        vec!["header:authorization".to_string(), "body".to_string()]
    );

    let metadata =
        operation_auth_metadata(HttpMethod::Post, "/web/auth/bootstrap-session", &bootstrap);
    assert_eq!(metadata["browser_session_token_required"], true);
    assert_eq!(metadata["stored_cli_auth"], false);
}

#[test]
fn api_coverage_matches_request_log_to_openapi_operations() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let request_log = temp_dir.path().join("requests.jsonl");
    crate::request_log::append_request_record_at(
        &request_log,
        &json!({
            "timestamp": "2026-05-07T00:00:00Z",
            "kind": "raw",
            "method": "GET",
            "path": "/views/projects/project-1/incidents",
            "status": 200,
            "success": true,
            "request_id": "req_ok",
        }),
    )
    .expect("append first request");
    crate::request_log::append_request_record_at(
        &request_log,
        &json!({
            "timestamp": "2026-05-07T00:01:00Z",
            "kind": "raw",
            "method": "POST",
            "path": "/projects",
            "status": 500,
            "success": false,
            "request_id": "req_500",
            "operation_id": "post_projects",
        }),
    )
    .expect("append second request");

    let spec = json!({
        "paths": {
            "/views/projects/{projectId}/incidents": {
                "get": {
                    "operationId": "get_project_incidents"
                }
            },
            "/projects": {
                "post": {
                    "operationId": "post_projects"
                }
            },
            "/health": {
                "get": {
                    "operationId": "get_health"
                }
            }
        }
    });

    let coverage =
        api_coverage(&spec, &request_log, 100, "http://localhost:3000").expect("coverage");
    assert_eq!(coverage["total_operations"], 3);
    assert_eq!(coverage["no_hit_count"], 1);
    assert_eq!(coverage["no_2xx_count"], 1);
    assert_eq!(coverage["write_no_2xx_count"], 1);
    assert_eq!(
        coverage["no_2xx"][0]["operation_id"],
        json!("post_projects")
    );
    assert_eq!(coverage["no_hit"][0]["operation_id"], json!("get_health"));
}

#[test]
fn synthesizes_missing_operation_ids() {
    assert_eq!(
        synthetic_operation_id(HttpMethod::Post, "/web/auth/bootstrap-session"),
        "post_web_auth_bootstrap_session"
    );
}

#[test]
fn builds_public_incidents_workflow_request() {
    let request = incidents_request(&IncidentsArgs {
        limit: Some(5),
        ..incidents_args()
    })
    .unwrap();

    assert_eq!(request.path, "/views/public/incidents");
    assert!(!request.require_auth);
    assert_eq!(request.query, vec![("limit".to_string(), "5".to_string())]);
}

#[test]
fn builds_project_incidents_workflow_request() {
    let request = incidents_request(&IncidentsArgs {
        project_id: Some("project-1".to_string()),
        assertion_id: Some("assertion-1".to_string()),
        environment: Some("production".to_string()),
        limit: Some(10),
        ..incidents_args()
    })
    .unwrap();

    assert_eq!(request.path, "/views/projects/project-1/incidents");
    assert!(request.require_auth);
    assert!(
        request
            .query
            .contains(&("limit".to_string(), "10".to_string()))
    );
    assert!(
        request
            .query
            .contains(&("assertionId".to_string(), "assertion-1".to_string()))
    );
    assert!(
        request
            .query
            .contains(&("environment".to_string(), "production".to_string()))
    );
}

#[test]
fn builds_project_incident_stats_workflow_request() {
    let request = incidents_request(&IncidentsArgs {
        project_id: Some("project-1".to_string()),
        stats: true,
        ..incidents_args()
    })
    .unwrap();

    assert_eq!(request.path, "/projects/project-1/incidents/stats");
    assert_eq!(request.method.openapi_key(), "get");
    assert!(request.require_auth);
}

#[test]
fn incident_detail_and_trace_require_auth() {
    let detail = incidents_request(&IncidentsArgs {
        incident_id: Some("incident-1".to_string()),
        ..incidents_args()
    })
    .unwrap();
    assert_eq!(detail.path, "/views/incidents/incident-1");
    assert!(detail.require_auth);

    let trace = incidents_request(&IncidentsArgs {
        incident_id: Some("incident-1".to_string()),
        tx_id: Some("tx-1".to_string()),
        ..incidents_args()
    })
    .unwrap();
    assert_eq!(
        trace.path,
        "/views/incidents/incident-1/transactions/tx-1/trace"
    );
    assert!(trace.require_auth);
}

#[test]
fn builds_incident_trace_retry_request() {
    let request = incidents_request(&IncidentsArgs {
        incident_id: Some("incident-1".to_string()),
        tx_id: Some("tx-1".to_string()),
        retry_trace: true,
        ..incidents_args()
    })
    .unwrap();

    assert_eq!(
        request.path,
        "/incidents/incident-1/transactions/tx-1/trace/retry"
    );
    assert_eq!(request.method.openapi_key(), "post");
    assert_eq!(request.body.as_deref(), Some("{}"));
    assert!(request.require_auth);
}

#[test]
fn incident_detail_next_action_uses_invalidating_transaction_id() {
    let next_actions = incidents_next_actions(
        &json!({
            "data": {
                "invalidating_transactions": [{
                    "id": "invalidating-tx-1",
                    "transaction_hash": "0xde68add41baa3f8541de6acd572ad61b1dae78c4916412d8f273cc6f57af540b"
                }]
            }
        }),
        &IncidentsArgs {
            incident_id: Some("incident-1".to_string()),
            ..incidents_args()
        },
        vec!["fallback".to_string()],
    );

    assert_eq!(
        next_actions,
        vec![
            "pcl incidents --incident-id incident-1 --tx-id invalidating-tx-1".to_string(),
            "pcl incidents --limit 5".to_string(),
        ]
    );
}

#[tokio::test]
async fn paginates_incident_list_workflows() {
    let mut server = mockito::Server::new_async().await;
    let page_1 = server
        .mock("GET", "/api/v1/views/public/incidents")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("page".into(), "1".into()),
            mockito::Matcher::UrlEncoded("limit".into(), "2".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"incidents":[{"id":"i1"},{"id":"i2"}]}"#)
        .create_async()
        .await;
    let page_2 = server
        .mock("GET", "/api/v1/views/public/incidents")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("page".into(), "2".into()),
            mockito::Matcher::UrlEncoded("limit".into(), "2".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"incidents":[{"id":"i3"}]}"#)
        .create_async()
        .await;
    let api = test_api(server.url(), true);
    let request = WorkflowRequest::get("/views/public/incidents", false, Vec::<String>::new());
    let mut config = CliConfig::default();
    let cli_args = CliArgs::default();

    let data = api
        .call_workflow_paginated(
            &mut config,
            &cli_args,
            request,
            WorkflowPaginationOptions {
                item_field: "incidents",
                start_page: 1,
                limit: 2,
                max_pages: 5,
            },
            test_request_log_path(),
        )
        .await
        .unwrap();

    assert_eq!(data["count"], 3);
    assert_eq!(data["pages_fetched"], 2);
    assert_eq!(data["items"][2]["id"], "i3");
    page_1.assert_async().await;
    page_2.assert_async().await;
}

#[tokio::test]
async fn incident_workflow_pagination_rejects_zero_limit() {
    let api = test_api("https://app.phylax.systems", true);
    let request = WorkflowRequest::get("/views/public/incidents", false, Vec::<String>::new());
    let mut config = CliConfig::default();
    let cli_args = CliArgs::default();

    let error = api
        .call_workflow_paginated(
            &mut config,
            &cli_args,
            request,
            WorkflowPaginationOptions {
                item_field: "incidents",
                start_page: 1,
                limit: 0,
                max_pages: 5,
            },
            test_request_log_path(),
        )
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("--limit must be greater than zero")
    );
}

#[tokio::test]
async fn authenticated_project_slug_resolution_attaches_auth() {
    let mut server = mockito::Server::new_async().await;
    let project_id = "550e8400-e29b-41d4-a716-446655440000";
    let resolve = server
        .mock("GET", "/api/v1/projects/resolve/private-slug")
        .match_header("authorization", "Bearer access-token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(r#"{{"project_id":"{project_id}"}}"#))
        .expect(1)
        .create_async()
        .await;
    let detail = server
        .mock("GET", format!("/api/v1/projects/{project_id}").as_str())
        .match_header("authorization", "Bearer access-token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"project_id":"{project_id}","slug":"private-slug"}}"#
        ))
        .expect(1)
        .create_async()
        .await;
    let api = test_api(server.url(), false);
    let mut config = valid_auth_config("access-token", "refresh-token");
    let request = WorkflowRequest::get("/projects/private-slug", true, Vec::<String>::new());

    let result = api
        .call_workflow_result(
            &mut config,
            &CliArgs::default(),
            &request,
            test_request_log_path(),
        )
        .await
        .unwrap();

    assert_eq!(result.body["slug"], "private-slug");
    assert_eq!(result.request["path"], format!("/projects/{project_id}"));
    resolve.assert_async().await;
    detail.assert_async().await;
}

#[tokio::test]
async fn project_slug_resolution_errors_preserve_http_metadata() {
    let mut server = mockito::Server::new_async().await;
    let resolve = server
        .mock("GET", "/api/v1/projects/resolve/missing-slug")
        .match_header("authorization", "Bearer access-token")
        .with_status(404)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-resolve-404")
        .with_body(r#"{"error":"Project not found"}"#)
        .expect(1)
        .create_async()
        .await;
    let api = test_api(server.url(), false);
    let mut config = valid_auth_config("access-token", "refresh-token");
    let request = WorkflowRequest::get("/projects/missing-slug", true, Vec::<String>::new());

    let error = api
        .call_workflow_result(
            &mut config,
            &CliArgs::default(),
            &request,
            test_request_log_path(),
        )
        .await
        .unwrap_err();

    let ApiCommandError::HttpStatus {
        method,
        path,
        status,
        request_id,
        body,
    } = &error
    else {
        panic!("expected HTTP status error, got {error:?}");
    };
    assert_eq!(*method, "GET");
    assert_eq!(path, "/projects/resolve/missing-slug");
    assert_eq!(*status, 404);
    assert_eq!(request_id.as_deref(), Some("req-resolve-404"));
    assert_eq!(body["error"], "Project not found");
    assert_eq!(
        error.json_envelope()["error"]["request_id"],
        "req-resolve-404"
    );
    assert_eq!(error.json_envelope()["http_status"], 404);
    resolve.assert_async().await;
}

#[tokio::test]
async fn openapi_discovery_errors_preserve_http_metadata() {
    let mut server = mockito::Server::new_async().await;
    let openapi = server
        .mock("GET", "/api/v1/openapi")
        .with_status(503)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-openapi-503")
        .with_body(r#"{"error":"OpenAPI unavailable"}"#)
        .expect(1)
        .create_async()
        .await;
    let api = test_api(server.url(), true);

    let error = api.fetch_openapi(&CliConfig::default()).await.unwrap_err();

    let ApiCommandError::HttpStatus {
        method,
        path,
        status,
        request_id,
        body,
    } = &error
    else {
        panic!("expected HTTP status error, got {error:?}");
    };
    assert_eq!(*method, "GET");
    assert_eq!(path, "/openapi");
    assert_eq!(*status, 503);
    assert_eq!(request_id.as_deref(), Some("req-openapi-503"));
    assert_eq!(body["error"], "OpenAPI unavailable");
    assert_eq!(
        error.json_envelope()["error"]["request_id"],
        "req-openapi-503"
    );
    openapi.assert_async().await;
}

#[tokio::test]
async fn public_workflows_do_not_attach_expired_stored_tokens() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/api/v1/health")
        .match_header("authorization", Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"healthy":true}"#)
        .create_async()
        .await;
    let api = test_api(server.url(), false);
    let mut config = expired_auth_config("expired-token", "refresh-token");

    let output = api
        .run_workflow(
            &mut config,
            &CliArgs::default(),
            "search",
            WorkflowRequest::get("/health", false, vec!["pcl search --health".to_string()]),
            test_request_log_path(),
        )
        .await
        .unwrap();

    assert_eq!(output["status"], "ok");
    assert_eq!(output["request"]["auth"]["will_attach_stored_token"], false);
    mock.assert_async().await;
}

#[tokio::test]
async fn public_raw_calls_do_not_attach_expired_stored_tokens() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/api/v1/views/public/incidents")
        .match_header("authorization", Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"incidents":[]}"#)
        .create_async()
        .await;
    let api = test_api(server.url(), false);
    let mut config = expired_auth_config("expired-token", "refresh-token");

    let input = ApiRequestInput {
        method: HttpMethod::Get,
        path: "/views/public/incidents",
        query: &[],
        header: &[],
        body: None,
        body_file: None,
        field: &[],
        require_auth: api
            .raw_call_requires_auth(HttpMethod::Get, "/views/public/incidents")
            .unwrap(),
    };
    let output = api
        .call_api(
            &mut config,
            &CliArgs::default(),
            input,
            test_request_log_path(),
        )
        .await
        .unwrap();

    assert_eq!(output["response"]["status"], 200);
    mock.assert_async().await;
}

#[tokio::test]
async fn authenticated_workflow_retries_once_after_refresh_on_401() {
    let mut server = mockito::Server::new_async().await;
    let first_attempt = server
        .mock("GET", "/api/v1/web/auth/me")
        .match_header("authorization", "Bearer old_access")
        .with_status(401)
        .with_header("content-type", "application/json")
        .with_body(r#"{"code":"TOKEN_EXPIRED","error":"expired"}"#)
        .expect(1)
        .create_async()
        .await;
    let refresh = server
        .mock("POST", "/api/v1/auth/refresh")
        .match_body(Matcher::Json(json!({ "refresh_token": "old_refresh" })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"token":"new_access","refresh_token":"new_refresh","expires_at":"2030-01-01T00:00:00Z","refresh_expires_at":"2030-02-01T00:00:00Z"}"#,
        )
        .expect(1)
        .create_async()
        .await;
    let retry = server
        .mock("GET", "/api/v1/web/auth/me")
        .match_header("authorization", "Bearer new_access")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-after-refresh")
        .with_body(r#"{"email":"agent@example.com"}"#)
        .expect(1)
        .create_async()
        .await;
    let api = test_api(server.url(), false);
    let temp_dir = tempfile::tempdir().unwrap();
    let cli_args = CliArgs {
        config_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };
    let mut config = valid_auth_config("old_access", "old_refresh");
    config.write_to_file(&cli_args).unwrap();
    let request = WorkflowRequest::get("/web/auth/me", true, Vec::<String>::new());

    let result = api
        .call_workflow_result(&mut config, &cli_args, &request, test_request_log_path())
        .await
        .unwrap();

    assert_eq!(result.body["email"], "agent@example.com");
    assert_eq!(result.request["retried_after_refresh"], true);
    assert_eq!(config.auth.as_ref().unwrap().refresh_token, "new_refresh");
    first_attempt.assert_async().await;
    refresh.assert_async().await;
    retry.assert_async().await;
}

#[tokio::test]
async fn raw_401_preserves_original_error_when_refresh_endpoint_is_missing() {
    let mut server = mockito::Server::new_async().await;
    let original = server
        .mock(
            "GET",
            "/api/v1/projects/550e8400-e29b-41d4-a716-446655440000/incidents/stats",
        )
        .match_header("authorization", "Bearer old_access")
        .with_status(401)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-original-401")
        .with_body(r#"{"code":"PROJECT_ACCESS_DENIED","error":"project not owned"}"#)
        .expect(1)
        .create_async()
        .await;
    let refresh = server
        .mock("POST", "/api/v1/auth/refresh")
        .match_body(Matcher::Json(json!({ "refresh_token": "old_refresh" })))
        .with_status(404)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-refresh-404")
        .with_body(r#"{"error":"Not Found"}"#)
        .expect(1)
        .create_async()
        .await;
    let api = test_api(server.url(), false);
    let temp_dir = tempfile::tempdir().unwrap();
    let cli_args = CliArgs {
        config_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };
    let mut config = valid_auth_config("old_access", "old_refresh");
    config.write_to_file(&cli_args).unwrap();
    let input = ApiRequestInput {
        method: HttpMethod::Get,
        path: "/projects/550e8400-e29b-41d4-a716-446655440000/incidents/stats",
        query: &[],
        header: &[],
        body: None,
        body_file: None,
        field: &[],
        require_auth: true,
    };

    let error = api
        .call_api(&mut config, &cli_args, input, test_request_log_path())
        .await
        .unwrap_err();

    let ApiCommandError::HttpStatus {
        status,
        request_id,
        body,
        ..
    } = error
    else {
        panic!("expected original HTTP status error");
    };
    assert_eq!(status, 401);
    assert_eq!(request_id.as_deref(), Some("req-original-401"));
    assert_eq!(body["code"], "PROJECT_ACCESS_DENIED");
    assert_eq!(body["error"], "project not owned");
    assert!(!api.refresh_after_401.get());
    original.assert_async().await;
    refresh.assert_async().await;
}

#[tokio::test]
async fn incident_stats_401_propagates_original_http_error() {
    let mut server = mockito::Server::new_async().await;
    let project_id = "550e8400-e29b-41d4-a716-446655440000";
    let original = server
        .mock(
            "GET",
            format!("/api/v1/projects/{project_id}/incidents/stats").as_str(),
        )
        .match_header("authorization", "Bearer old_access")
        .with_status(401)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-stats-401")
        .with_body(r#"{"code":"PROJECT_ACCESS_DENIED","error":"project not owned"}"#)
        .expect(1)
        .create_async()
        .await;
    let refresh = server
        .mock("POST", "/api/v1/auth/refresh")
        .match_body(Matcher::Json(json!({ "refresh_token": "old_refresh" })))
        .with_status(404)
        .with_header("content-type", "application/json")
        .with_body(r#"{"error":"Not Found"}"#)
        .expect(1)
        .create_async()
        .await;
    let api = test_api(server.url(), false);
    let temp_dir = tempfile::tempdir().unwrap();
    let cli_args = CliArgs {
        config_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };
    let mut config = valid_auth_config("old_access", "old_refresh");
    config.write_to_file(&cli_args).unwrap();
    let args = IncidentsArgs {
        project_id: Some(project_id.to_string()),
        stats: true,
        ..incidents_args()
    };

    let error = api
        .run_incidents(&mut config, &cli_args, &args, test_request_log_path())
        .await
        .unwrap_err();

    let ApiCommandError::HttpStatus {
        status,
        request_id,
        body,
        ..
    } = error
    else {
        panic!("expected stats HTTP status error");
    };
    assert_eq!(status, 401);
    assert_eq!(request_id.as_deref(), Some("req-stats-401"));
    assert_eq!(body["code"], "PROJECT_ACCESS_DENIED");
    original.assert_async().await;
    refresh.assert_async().await;
}
#[test]
fn builds_project_create_body_from_typed_flags() {
    let request = projects_request(&ProjectsArgs {
        create: true,
        project_name: Some("Demo".to_string()),
        chain_id: Some(1),
        is_private: Some(false),
        ..projects_args()
    })
    .unwrap();

    assert_eq!(request.path, "/projects");
    assert_eq!(request.method.openapi_key(), "post");
    assert_eq!(request.operation_id, Some("post_projects"));
    assert_eq!(
        serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap(),
        json!({
            "project_name": "Demo",
            "chain_id": 1,
            "is_private": false
        })
    );
}

#[test]
fn builds_assertion_lifecycle_requests() {
    let registered = assertions_request(&AssertionsArgs {
        registered: true,
        ..assertions_args(Some("project-1"))
    })
    .unwrap();
    assert_eq!(registered.path, "/projects/project-1/registered-assertions");

    let remove = assertions_request(&AssertionsArgs {
        remove_calldata: true,
        ..assertions_args(Some("project-1"))
    })
    .unwrap();
    assert_eq!(
        remove.path,
        "/projects/project-1/remove-assertions-calldata"
    );
}

#[test]
fn submitted_assertion_workflows_are_removed() {
    let error = assertions_request(&AssertionsArgs {
        submitted: true,
        ..assertions_args(Some("project-1"))
    })
    .unwrap_err();

    match error {
        ApiCommandError::InvalidWorkflow { message } => {
            assert!(message.contains("Submitted assertions have been removed"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn builds_adopter_assertion_lookup_request() {
    let request = assertions_request(&AssertionsArgs {
        adopter_address: Some("0xabc".to_string()),
        network: Some("1".to_string()),
        environment: Some("production".to_string()),
        include_onchain_only: Some(true),
        ..assertions_args(None)
    })
    .unwrap();

    assert_eq!(request.path, "/assertions");
    assert!(!request.require_auth);
    assert_eq!(
        request.query,
        vec![
            ("adopter_address".to_string(), "0xabc".to_string()),
            ("network".to_string(), "1".to_string()),
            ("environment".to_string(), "production".to_string()),
            ("include_onchain_only".to_string(), "true".to_string()),
        ]
    );
}

#[test]
fn project_assertions_require_project_id() {
    let error = assertions_request(&assertions_args(None)).unwrap_err();
    assert!(error.to_string().contains("--project-id is required"));
}

#[test]
fn protocol_manager_transfer_calldata_uses_new_manager_query() {
    let request = protocol_manager_request(&ProtocolManagerArgs {
        transfer_calldata: true,
        new_manager: Some("0xmanager".to_string()),
        ..protocol_manager_args()
    })
    .unwrap();

    assert_eq!(
        request.path,
        "/projects/project-1/protocol-manager/transfer-calldata"
    );
    assert_eq!(
        request.query,
        vec![("new_manager".to_string(), "0xmanager".to_string())]
    );
}

#[test]
fn protocol_manager_transfer_calldata_requires_new_manager() {
    let error = protocol_manager_request(&ProtocolManagerArgs {
        transfer_calldata: true,
        ..protocol_manager_args()
    })
    .unwrap_err();

    assert!(error.to_string().contains("--new-manager is required"));
}

#[test]
fn saved_projects_require_and_send_user_id() {
    let error = projects_request(&ProjectsArgs {
        saved: true,
        ..projects_args()
    })
    .unwrap_err();
    assert!(error.to_string().contains("--user-id is required"));

    let request = projects_request(&ProjectsArgs {
        saved: true,
        user_id: Some("user-1".to_string()),
        ..projects_args()
    })
    .unwrap();
    assert_eq!(request.path, "/projects/saved");
    assert_eq!(request.operation_id, Some("get_projects_saved"));
    assert_eq!(
        request.query,
        vec![("user_id".to_string(), "user-1".to_string())]
    );
}

#[test]
fn projects_mine_uses_authenticated_home_view() {
    let request = projects_request(&ProjectsArgs {
        mine: true,
        ..projects_args()
    })
    .unwrap();

    assert_eq!(request.path, "/views/projects/home");
    assert_eq!(request.method.openapi_key(), "get");
    assert_eq!(request.operation_id, Some("get_views_projects_home"));
    assert!(request.require_auth);
    assert_eq!(
        request.next_actions,
        vec![
            "pcl account".to_string(),
            "pcl projects saved --user-id <user-id>".to_string()
        ]
    );
}

#[test]
fn contracts_unassigned_require_and_send_manager() {
    let error = contracts_request(&ContractsArgs {
        unassigned: true,
        ..contracts_args()
    })
    .unwrap_err();
    assert!(error.to_string().contains("--manager is required"));

    let request = contracts_request(&ContractsArgs {
        unassigned: true,
        manager: Some("0xmanager".to_string()),
        ..contracts_args()
    })
    .unwrap();
    assert_eq!(request.path, "/assertion_adopters/no-project");
    assert_eq!(
        request.query,
        vec![("manager".to_string(), "0xmanager".to_string())]
    );
}

#[test]
fn contracts_remove_calldata_requires_and_sends_assertion_ids() {
    let error = contracts_request(&ContractsArgs {
        remove_calldata: true,
        aa_address: Some("0xabc".to_string()),
        ..contracts_args()
    })
    .unwrap_err();
    assert!(error.to_string().contains("--assertion-id is required"));

    let request = contracts_request(&ContractsArgs {
        remove_calldata: true,
        aa_address: Some("0xabc".to_string()),
        network: Some("31337".to_string()),
        environment: Some("production".to_string()),
        assertion_ids: vec!["assertion-1".to_string(), "assertion-2".to_string()],
        ..contracts_args()
    })
    .unwrap();
    assert_eq!(
        request.path,
        "/assertion_adopters/0xabc/remove-assertions-calldata"
    );
    assert_eq!(
        request.query,
        vec![
            ("network".to_string(), "31337".to_string()),
            ("environment".to_string(), "production".to_string()),
            ("assertion_ids".to_string(), "assertion-1".to_string()),
            ("assertion_ids".to_string(), "assertion-2".to_string()),
        ]
    );
}

#[test]
fn release_deploy_calldata_requires_and_sends_signer_address() {
    let error = releases_request(&ReleasesArgs {
        release_id: Some("release-1".to_string()),
        deploy_calldata: true,
        ..release_args()
    })
    .unwrap_err();
    assert!(error.to_string().contains("--signer-address is required"));

    let request = releases_request(&ReleasesArgs {
        release_id: Some("release-1".to_string()),
        signer_address: Some("0xsigner".to_string()),
        deploy_calldata: true,
        ..release_args()
    })
    .unwrap();
    assert_eq!(
        request.path,
        "/projects/project-1/releases/release-1/deploy-calldata"
    );
    assert_eq!(
        request.operation_id,
        Some("get_projects_project_id_releases_release_id_deploy_calldata")
    );
    assert_eq!(
        request.query,
        vec![("signerAddress".to_string(), "0xsigner".to_string())]
    );
}

#[test]
fn release_check_progress_and_retry_are_first_class_workflows() {
    let progress = releases_request(&ReleasesArgs {
        release_id: Some("release-1".to_string()),
        backtest_progress: true,
        ..release_args()
    })
    .unwrap();
    assert_eq!(
        progress.path,
        "/projects/project-1/releases/release-1/backtest-progress"
    );
    assert_eq!(
        progress.operation_id,
        Some("get_projects_project_id_releases_release_id_backtest_progress")
    );
    assert!(progress.body.is_none());

    let error = releases_request(&ReleasesArgs {
        release_id: Some("release-1".to_string()),
        retry_check: true,
        ..release_args()
    })
    .unwrap_err();
    assert!(error.to_string().contains("--check-id is required"));

    let retry = releases_request(&ReleasesArgs {
        release_id: Some("release-1".to_string()),
        check_id: Some("check-1".to_string()),
        retry_check: true,
        ..release_args()
    })
    .unwrap();
    assert_eq!(
        retry.path,
        "/projects/project-1/releases/release-1/checks/check-1/retry"
    );
    assert_eq!(
        retry.operation_id,
        Some("post_projects_project_id_releases_release_id_checks_check_id_retry")
    );
    assert_eq!(retry.method, HttpMethod::Post);
    assert_eq!(retry.body, Some(json!({}).to_string()));
}

#[test]
fn raw_operations_advertise_workflow_alternatives_when_available() {
    let release = workflow_alternatives(
        HttpMethod::Get,
        "/projects/{project_id}/releases/{release_id}/backtest-progress",
    );
    assert!(release.iter().any(|alternative| {
        alternative["workflow"] == "releases"
            && alternative["action"] == "backtest_progress"
            && alternative["example"]
                .as_str()
                .is_some_and(|example| example.contains("backtest-progress"))
    }));

    let integration = workflow_alternatives(
        HttpMethod::Post,
        "/projects/{project_id}/integrations/slack/test",
    );
    assert!(integration.iter().any(|alternative| {
        alternative["workflow"] == "integrations"
            && alternative["action"] == "test"
            && alternative["example"]
                .as_str()
                .is_some_and(|example| example.contains("--provider slack --test"))
    }));

    let legacy = workflow_alternatives(HttpMethod::Get, "/public/incidents");
    assert!(legacy.iter().any(|alternative| {
        alternative["workflow"] == "incidents"
            && alternative["example"] == "pcl incidents --limit 5 --toon"
    }));

    let project_detail = workflow_alternatives(HttpMethod::Get, "/projects/{project_id}");
    assert_eq!(project_detail.len(), 1);
    assert_eq!(project_detail[0]["workflow"], "projects");
    assert_eq!(project_detail[0]["action"], "detail");
    assert_eq!(
        project_detail[0]["example"],
        "pcl projects show <project-ref> --toon"
    );

    let saved_delete = workflow_alternatives(HttpMethod::Delete, "/projects/saved");
    assert_eq!(saved_delete.len(), 1);
    assert_eq!(saved_delete[0]["workflow"], "projects");
    assert_eq!(saved_delete[0]["action"], "unsave");
    assert_eq!(
        saved_delete[0]["example"],
        "pcl projects unsave <project-ref> --toon"
    );

    let project_literal = workflow_alternatives(HttpMethod::Get, "/projects/project-1");
    assert_eq!(project_literal.len(), 1);
    assert_eq!(project_literal[0]["action"], "detail");
}

#[test]
fn raw_api_policy_classifies_only_real_raw_fallbacks() {
    let assert_policy = |method, path, operation, has_workflow, policy| {
        assert_eq!(
            raw_api_use(method, path, &operation, has_workflow)["policy"],
            policy
        );
    };
    let browser_bridge = json!({
        "parameters": [
            {"name": "authorization", "in": "header", "required": true}
        ]
    });

    assert_policy(
        HttpMethod::Post,
        "/backtesting/events",
        json!({}),
        false,
        "internal_service",
    );
    assert_policy(
        HttpMethod::Post,
        "/web/auth/bootstrap-session",
        browser_bridge,
        false,
        "browser_session_bridge",
    );
    assert_policy(
        HttpMethod::Get,
        "/new-endpoint",
        json!({}),
        false,
        "debug_escape_hatch",
    );
    assert_policy(
        HttpMethod::Get,
        "/public/incidents",
        json!({}),
        true,
        "prefer_workflow",
    );
}

#[test]
fn protocol_manager_nonce_requires_and_sends_address() {
    let error = protocol_manager_request(&ProtocolManagerArgs {
        nonce: true,
        ..protocol_manager_args()
    })
    .unwrap_err();
    assert!(error.to_string().contains("--address is required"));

    let request = protocol_manager_request(&ProtocolManagerArgs {
        nonce: true,
        address: Some("0xmanager".to_string()),
        chain_id: Some(1),
        ..protocol_manager_args()
    })
    .unwrap();
    assert_eq!(request.path, "/projects/project-1/protocol-manager/nonce");
    assert_eq!(
        request.query,
        vec![
            ("address".to_string(), "0xmanager".to_string()),
            ("chain_id".to_string(), "1".to_string()),
        ]
    );
}

#[test]
fn write_actions_require_target_identifiers() {
    let project_error = projects_request(&ProjectsArgs {
        save: true,
        ..projects_args()
    })
    .unwrap_err();
    assert!(
        project_error
            .to_string()
            .contains("--project-id is required")
    );

    let release_error = releases_request(&ReleasesArgs {
        deploy: true,
        ..release_args()
    })
    .unwrap_err();
    assert!(
        release_error
            .to_string()
            .contains("--release-id is required")
    );

    let token_error = access_request(&AccessArgs {
        token: None,
        accept: true,
        ..access_args()
    })
    .unwrap_err();
    assert!(token_error.to_string().contains("--token is required"));

    let invitation_error = access_request(&AccessArgs {
        resend: true,
        ..access_args()
    })
    .unwrap_err();
    assert!(
        invitation_error
            .to_string()
            .contains("--invitation-id is required")
    );

    let member_error = access_request(&AccessArgs {
        update_role: true,
        ..access_args()
    })
    .unwrap_err();
    assert!(
        member_error
            .to_string()
            .contains("--member-user-id is required")
    );
}

#[test]
fn builds_account_workflow_requests() {
    let me = account_request(&AccountArgs {
        me: true,
        accept_terms: false,
        logout: false,
        body: None,
        field: Vec::new(),
        body_file: None,
        body_template: false,
    })
    .unwrap();
    assert_eq!(me.path, "/web/auth/me");
    assert_eq!(me.method.openapi_key(), "get");

    let accept_terms = account_request(&AccountArgs {
        me: false,
        accept_terms: true,
        logout: false,
        body: None,
        field: Vec::new(),
        body_file: None,
        body_template: false,
    })
    .unwrap();
    assert_eq!(accept_terms.path, "/web/auth/accept-terms");
    assert_eq!(accept_terms.method.openapi_key(), "post");
    assert_eq!(accept_terms.body.as_deref(), Some("{}"));
}

#[test]
fn empty_object_workflows_send_body_by_default() {
    let accept = access_request(&AccessArgs {
        accept: true,
        token: Some("token-1".to_string()),
        ..access_args()
    })
    .unwrap();
    assert_eq!(accept.method.openapi_key(), "post");
    assert_eq!(accept.body.as_deref(), Some("{}"));

    let resend = access_request(&AccessArgs {
        resend: true,
        invitation_id: Some("invitation-1".to_string()),
        ..access_args()
    })
    .unwrap();
    assert_eq!(resend.method.openapi_key(), "post");
    assert_eq!(resend.body.as_deref(), Some("{}"));

    let retry_trace = incidents_request(&IncidentsArgs {
        incident_id: Some("incident-1".to_string()),
        tx_id: Some("tx-1".to_string()),
        retry_trace: true,
        ..incidents_args()
    })
    .unwrap();
    assert_eq!(retry_trace.method.openapi_key(), "post");
    assert_eq!(
        retry_trace.path,
        "/incidents/incident-1/transactions/tx-1/trace/retry"
    );
    assert_eq!(retry_trace.body.as_deref(), Some("{}"));

    let test = integrations_request(&IntegrationsArgs {
        project: Some("project-1".to_string()),
        provider: Some(IntegrationProvider::Slack),
        configure: false,
        test: true,
        delete: false,
        body: None,
        field: Vec::new(),
        body_file: None,
        body_template: false,
    })
    .unwrap();
    assert_eq!(test.method.openapi_key(), "post");
    assert_eq!(test.path, "/projects/project-1/integrations/slack/test");
    assert_eq!(test.body.as_deref(), Some("{}"));

    let delete = integrations_request(&IntegrationsArgs {
        project: Some("project-1".to_string()),
        provider: Some(IntegrationProvider::Pagerduty),
        configure: false,
        test: false,
        delete: true,
        body: None,
        field: Vec::new(),
        body_file: None,
        body_template: false,
    })
    .unwrap();
    assert_eq!(
        delete.next_actions,
        vec!["pcl integrations --project project-1 --provider pagerduty"]
    );
}

#[tokio::test]
async fn workflow_http_errors_include_response_body() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/api/v1/health")
        .with_status(422)
        .with_header("content-type", "application/json")
        .with_body(r#"{"message":"address is required","field":"address"}"#)
        .create_async()
        .await;
    let api = test_api(server.url(), true);
    let mut config = CliConfig::default();
    let request = WorkflowRequest::get("/health", false, Vec::<String>::new());

    let error = api
        .call_workflow_result(
            &mut config,
            &CliArgs::default(),
            &request,
            test_request_log_path(),
        )
        .await
        .unwrap_err();
    let ApiCommandError::HttpStatus {
        method,
        path,
        status,
        request_id,
        body,
    } = &error
    else {
        panic!("expected HTTP status error, got {error:?}");
    };

    assert_eq!(*method, "GET");
    assert_eq!(path, "/health");
    assert_eq!(*status, 422);
    assert_eq!(request_id, &None);
    assert_eq!(body["field"], "address");
    assert_eq!(error.code(), "api.validation_failed");
    assert_eq!(
        error.json_envelope()["error"]["http"]["body"]["message"],
        "address is required"
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn workflow_success_envelopes_include_request_provenance() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/api/v1/health")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-workflow-123")
        .with_body(r#"{"ok":true}"#)
        .create_async()
        .await;
    let api = test_api(server.url(), true);
    let request = WorkflowRequest::get("/health", false, vec!["next".to_string()]);
    let mut config = CliConfig::default();

    let envelope = api
        .run_workflow(
            &mut config,
            &CliArgs::default(),
            "search",
            request,
            test_request_log_path(),
        )
        .await
        .unwrap();

    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["ok"], true);
    assert_eq!(envelope["request"]["method"], "GET");
    assert_eq!(envelope["request"]["path"], "/health");
    assert_eq!(envelope["request"]["auth"]["required"], false);
    assert_eq!(envelope["response"]["status"], 200);
    assert_eq!(envelope["response"]["request_id"], "req-workflow-123");
    assert!(envelope["response"]["fetched_at"].as_str().is_some());
    assert_eq!(envelope["next_actions"], json!(["next"]));
    mock.assert_async().await;
}

#[tokio::test]
async fn raw_api_call_accepts_inline_query_strings() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/api/v1/projects/project-1/incidents")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("environment".into(), "production".into()),
            mockito::Matcher::UrlEncoded("limit".into(), "50".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"ok":true}"#)
        .create_async()
        .await;
    let api = test_api(server.url(), true);
    let mut config = CliConfig::default();

    let response = api
        .call_api(
            &mut config,
            &CliArgs::default(),
            ApiRequestInput {
                method: HttpMethod::Get,
                path: "/projects/project-1/incidents?environment=production",
                query: &["limit=50".to_string()],
                header: &[],
                body: None,
                body_file: None,
                field: &[],
                require_auth: false,
            },
            test_request_log_path(),
        )
        .await
        .unwrap();

    assert_eq!(response["request"]["path"], "/projects/project-1/incidents");
    assert_eq!(
        response["request"]["query"],
        json!([
            {"name": "environment", "value": "production"},
            {"name": "limit", "value": "50"}
        ])
    );
    assert_eq!(response["response"]["body"]["ok"], true);
    mock.assert_async().await;
}
#[tokio::test]
async fn raw_api_call_paginates_any_array_response() {
    let mut server = mockito::Server::new_async().await;
    let page_1 = server
        .mock("GET", "/api/v1/views/public/incidents")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("environment".into(), "production".into()),
            mockito::Matcher::UrlEncoded("page".into(), "1".into()),
            mockito::Matcher::UrlEncoded("limit".into(), "2".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"incidents":[{"id":"i1"},{"id":"i2"}]}"#)
        .create_async()
        .await;
    let page_2 = server
        .mock("GET", "/api/v1/views/public/incidents")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("environment".into(), "production".into()),
            mockito::Matcher::UrlEncoded("page".into(), "2".into()),
            mockito::Matcher::UrlEncoded("limit".into(), "2".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"incidents":[{"id":"i3"}]}"#)
        .create_async()
        .await;
    let api = test_api(server.url(), true);
    let mut config = CliConfig::default();

    let response = api
        .call_api_paginated(
            &mut config,
            &CliArgs::default(),
            ApiRequestInput {
                method: HttpMethod::Get,
                path: "/views/public/incidents?environment=production",
                query: &[],
                header: &[],
                body: None,
                body_file: None,
                field: &[],
                require_auth: false,
            },
            RawPaginationOptions {
                item_field: "incidents",
                start_page: 1,
                limit: 2,
                page_param: "page",
                limit_param: "limit",
                max_pages: 5,
            },
            test_request_log_path(),
        )
        .await
        .unwrap();

    assert_eq!(response["count"], 3);
    assert_eq!(response["pages_fetched"], 2);
    assert_eq!(response["items"][2]["id"], "i3");
    assert_eq!(
        response["request"]["query"],
        json!([{"name": "environment", "value": "production"}])
    );
    assert_eq!(response["request"]["pagination"]["field"], "incidents");
    page_1.assert_async().await;
    page_2.assert_async().await;
}

#[tokio::test]
async fn raw_api_call_pagination_supports_custom_param_names() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/api/v1/custom")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("p".into(), "3".into()),
            mockito::Matcher::UrlEncoded("per_page".into(), "10".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"items":[{"id":"i1"}]}"#)
        .create_async()
        .await;
    let api = test_api(server.url(), true);
    let mut config = CliConfig::default();

    let response = api
        .call_api_paginated(
            &mut config,
            &CliArgs::default(),
            ApiRequestInput {
                method: HttpMethod::Get,
                path: "/custom",
                query: &[],
                header: &[],
                body: None,
                body_file: None,
                field: &[],
                require_auth: false,
            },
            RawPaginationOptions {
                item_field: "items",
                start_page: 3,
                limit: 10,
                page_param: "p",
                limit_param: "per_page",
                max_pages: 1,
            },
            test_request_log_path(),
        )
        .await
        .unwrap();

    assert_eq!(response["count"], 1);
    assert_eq!(response["request"]["pagination"]["page_param"], "p");
    assert_eq!(response["request"]["pagination"]["limit_param"], "per_page");
    mock.assert_async().await;
}

#[tokio::test]
async fn raw_api_call_pagination_rejects_non_get_requests() {
    let api = test_api("https://api.example.com", true);
    let mut config = CliConfig::default();

    let error = api
        .call_api_paginated(
            &mut config,
            &CliArgs::default(),
            ApiRequestInput {
                method: HttpMethod::Post,
                path: "/views/public/incidents",
                query: &[],
                header: &[],
                body: None,
                body_file: None,
                field: &[],
                require_auth: false,
            },
            RawPaginationOptions {
                item_field: "incidents",
                start_page: 1,
                limit: 50,
                page_param: "page",
                limit_param: "limit",
                max_pages: 100,
            },
            test_request_log_path(),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("--paginate is only supported"));
}

#[test]
fn writes_paginated_items_as_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("incidents.jsonl");
    let data = json!({
        "items": [
            {"id": "i1", "status": "ok"},
            {"id": "i2", "status": "failed"}
        ]
    });

    write_jsonl_items_output_file(&path, &data).unwrap();

    let output = fs::read_to_string(path).unwrap();
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(lines[0]).unwrap(),
        json!({"id": "i1", "status": "ok"})
    );
}

#[tokio::test]
async fn raw_api_call_errors_include_request_id() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/api/v1/health")
        .with_status(500)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-123")
        .with_body(r#"{"message":"server failed"}"#)
        .create_async()
        .await;
    let api = test_api(server.url(), true);
    let mut config = CliConfig::default();

    let error = api
        .call_api(
            &mut config,
            &CliArgs::default(),
            ApiRequestInput {
                method: HttpMethod::Get,
                path: "/health",
                query: &[],
                header: &[],
                body: None,
                body_file: None,
                field: &[],
                require_auth: false,
            },
            test_request_log_path(),
        )
        .await
        .unwrap_err();

    let ApiCommandError::HttpStatus {
        status,
        request_id,
        body,
        ..
    } = &error
    else {
        panic!("expected HTTP status error, got {error:?}");
    };
    assert_eq!(*status, 500);
    assert_eq!(request_id.as_deref(), Some("req-123"));
    assert_eq!(body["message"], "server failed");
    assert_eq!(error.json_envelope()["error"]["request_id"], "req-123");
    assert_eq!(error.json_envelope()["http_status"], 500);
    assert_eq!(error.json_envelope()["request_id"], "req-123");
    assert_eq!(
        error.json_envelope()["suggested_next_actions"],
        json!([
            "retry_later",
            "export_project_incidents_with_errors",
            "contact_platform_with_request_id"
        ])
    );
    assert!(
        error
            .next_actions()
            .iter()
            .any(|action| action.contains("req-123"))
    );
    mock.assert_async().await;
}

#[test]
fn mutating_server_errors_mark_outcome_ambiguous() {
    let error = ApiCommandError::HttpStatus {
        method: "POST",
        path: "/projects".to_string(),
        status: 500,
        request_id: Some("req-create-project".to_string()),
        body: Box::new(json!({"message": "created but failed after commit"})),
    };
    let envelope = error.json_envelope();

    assert_eq!(envelope["outcome_ambiguous"], true);
    assert_eq!(envelope["error"]["mutation"]["side_effecting"], true);
    assert_eq!(envelope["error"]["mutation"]["outcome_ambiguous"], true);
    assert_eq!(
        envelope["suggested_next_actions"],
        json!(["reconcile_mutation", "contact_platform_with_request_id"])
    );
    assert!(
        error
            .next_actions()
            .first()
            .is_some_and(|action| action.contains("Do not retry immediately"))
    );
}

#[test]
fn api_error_envelope_keeps_recoverable_inside_error_object() {
    let envelope = ApiCommandError::InvalidPath("health".to_string()).json_envelope();

    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["recoverable"], true);
    assert!(envelope.get("recoverable").is_none(), "{envelope}");
}

#[test]
fn forbidden_errors_preserve_permission_context() {
    let error = ApiCommandError::HttpStatus {
        method: "GET",
        path: "/system-status".to_string(),
        status: 403,
        request_id: Some("req-disabled".to_string()),
        body: Box::new(json!({"error": "System status checks are temporarily disabled"})),
    };
    let envelope = error.json_envelope();

    assert_eq!(
        envelope["suggested_next_actions"],
        json!(["check_permissions", "inspect_response_body"])
    );
    assert!(
        error
            .next_actions()
            .iter()
            .any(|action| action.contains("API-provided reason"))
    );
    assert!(
        !error
            .next_actions()
            .iter()
            .any(|action| action == "pcl auth refresh --toon")
    );
}

#[test]
fn body_templates_are_action_specific() {
    assert_eq!(
        project_body_template(&ProjectsArgs {
            create: true,
            ..projects_args()
        }),
        json!({
            "project_name": "<name>",
            "chain_id": 1,
            "project_description": "<description>",
            "profile_image_url": "https://example.com/project.png",
            "is_private": false
        })
    );
    assert_eq!(
        access_body_template(&AccessArgs {
            member_user_id: Some("user-1".to_string()),
            update_role: true,
            body_template: true,
            ..access_args()
        }),
        json!({ "role": "viewer" })
    );
    assert_eq!(
        release_body_template(&ReleasesArgs {
            release_id: Some("release-1".to_string()),
            deploy: true,
            body_template: true,
            ..release_args()
        }),
        json!({ "chainId": 1, "txHash": "0x..." })
    );
    assert_eq!(
        release_body_template(&ReleasesArgs {
            release_id: Some("release-1".to_string()),
            deploy_calldata: true,
            body_template: true,
            ..release_args()
        }),
        json!({})
    );
    assert_eq!(
        access_body_template(&AccessArgs {
            members: true,
            body_template: true,
            ..access_args()
        }),
        json!({})
    );
    assert_eq!(
        protocol_manager_body_template(&ProtocolManagerArgs {
            transfer_calldata: true,
            new_manager: Some("0xmanager".to_string()),
            ..protocol_manager_args()
        }),
        json!({})
    );
    assert_eq!(
        protocol_manager_body_template(&ProtocolManagerArgs {
            confirm_transfer: true,
            ..protocol_manager_args()
        }),
        json!({
            "body_variants": [
                {
                    "name": "direct",
                    "body": {
                        "mode": "direct",
                        "new_manager_address": "0x..."
                    }
                },
                {
                    "name": "onchain",
                    "body": {
                        "mode": "onchain",
                        "new_manager_address": "0x...",
                        "chain_id": 1,
                        "tx_hash": "0x..."
                    }
                }
            ]
        })
    );
    assert_eq!(
        body_template("pagerduty"),
        json!({ "routing_key": "<pagerduty-routing-key>", "enabled": true })
    );
}

#[test]
fn default_api_output_is_human_readable() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {"healthy": true},
            "next_actions": ["pcl api list"],
        }),
        false,
    )
    .unwrap();

    assert!(output.starts_with("OK\n"));
    assert_output_contains(&output, &["Healthy: yes", "Next:"]);
    assert_output_omits(&output, &["Schema: pcl.envelope.v1", "Details:"]);
}

#[test]
fn human_api_output_formats_incident_lists_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "data": {
                    "items": [
                        {
                            "id": "7dfe71ee-9d69-41bb-b33c-992c0fbd684f",
                            "title": "Removed invalid transaction",
                            "network": {"chainId": 59144, "name": "Linea Mainnet"},
                            "timestamp": "2026-05-06T14:01:54+00:00",
                            "referenceId": "c4f250"
                        }
                    ],
                    "pagination": {
                        "page": 1,
                        "limit": 20,
                        "total": 332,
                        "hasMore": true
                    }
                },
                "_meta": {
                    "sources": ["offchain"],
                    "fetchedAt": "2026-05-09T23:30:09.618Z"
                }
            },
            "request": {"method": "GET", "path": "/views/public/incidents"},
            "response": {"status": 200, "request_id": "req_123"},
            "next_actions": ["pcl incidents --incident-id 7dfe71ee-9d69-41bb-b33c-992c0fbd684f"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Incidents\n",
            "Showing 1 of 332 incidents on page 1 (limit 20)",
            "Updated: 2026-05-09 23:30",
            "Source: Phylax platform index",
            "Linea Mainnet (59144)",
            "Removed invalid transaction",
            "7dfe71ee-9d69-41bb-b33c-992c0fbd684f",
            "More results available. Try --page 2 --limit 20.",
            "Request ID: req_123 (HTTP 200)",
        ],
    );
    assert_output_omits(&output, &["Details:", "Request:\n", "Schema:"]);
}

#[test]
fn human_output_formats_empty_workflow_arrays_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": [],
            "request": {"method": "GET", "path": "/projects/project-1/releases"},
            "response": {"status": 200, "request_id": "req_empty"},
            "next_actions": ["pcl releases show project-1 <release-id>"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &["Releases\n", "Showing 0 releases", "No releases found."],
    );
    assert_output_omits(&output, &["<release-id>"]);
}

#[test]
fn human_output_keeps_placeholder_actions_when_other_collections_are_non_empty() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "projects": [
                    {"project_id": "project-1", "project_name": "Project 1"}
                ],
                "contracts": []
            },
            "request": {"method": "GET", "path": "/search"},
            "response": {"status": 200, "request_id": "req_search"},
            "next_actions": ["pcl contracts --project <project-ref>"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(&output, &["pcl contracts --project <project-ref>"]);
}

#[test]
fn human_output_keeps_safe_remove_calldata_actions() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "assertions": [
                    {"assertion_id": "assertion-1", "contract_name": "Guard"}
                ]
            },
            "request": {"method": "GET", "path": "/views/projects/project-1/assertions"},
            "response": {"status": 200, "request_id": "req_assertions"},
            "next_actions": ["pcl assertions --project-id project-1 --remove-calldata"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &["pcl assertions --project-id project-1 --remove-calldata"],
    );
}

#[test]
fn release_list_next_actions_use_returned_release_id() {
    let mut args = release_args();
    args.project = Some("project-1".to_string());
    let next_actions = releases_next_actions(
        &json!([
            {"id": "release-1", "status": "active"},
            {"id": "release-2", "status": "inactive"}
        ]),
        &args,
        vec!["pcl releases show project-1 <release-id>".to_string()],
    );

    assert_eq!(next_actions, vec!["pcl releases show project-1 release-1"]);
}

#[test]
fn contract_list_next_actions_use_returned_adopter_id() {
    let mut args = contracts_args();
    args.project = Some("project-1".to_string());
    let next_actions = contracts_next_actions(
        &json!({
            "data": {
                "contracts": [
                    {"id": "59144_0xabc", "address": "0xabc"},
                    {"id": "59144_0xdef", "address": "0xdef"}
                ]
            }
        }),
        &args,
        vec!["pcl contracts --project project-1 --adopter-id <adopter-id>".to_string()],
    );

    assert_eq!(
        next_actions,
        vec!["pcl contracts --project project-1 --adopter-id 59144_0xabc"]
    );
}

#[test]
fn transfer_list_next_actions_use_returned_transfer_id() {
    let args = transfers_args();
    let next_actions = transfers_next_actions(
        &json!({
            "incoming": {
                "project_transfers": [
                    {"id": "transfer-1", "project_id": "project-1"}
                ]
            },
            "outgoing": {"project_transfers": []}
        }),
        &args,
        vec!["pcl transfers --transfer-id <transfer-id>".to_string()],
    );

    assert_eq!(next_actions, vec!["pcl transfers --transfer-id transfer-1"]);
}

#[test]
fn protocol_manager_pending_next_actions_use_current_manager_address() {
    let args = protocol_manager_args();
    let next_actions = protocol_manager_next_actions(
        &json!({
            "has_pending_transfer": false,
            "current_manager_address": "0xmanager",
            "new_manager_address": null
        }),
        &args,
        vec![
            concat!(
                "pcl protocol-manager --project project-1 --nonce ",
                "--address <manager-address>"
            )
            .to_string(),
        ],
    );

    assert_eq!(
        next_actions[0],
        "pcl protocol-manager --project project-1 --nonce --address 0xmanager"
    );
    assert!(
        next_actions[1].contains("--new-manager <manager-address>"),
        "{next_actions:?}"
    );
}

#[test]
fn protocol_manager_pending_next_actions_offer_accept_calldata_when_pending() {
    let args = protocol_manager_args();
    let next_actions = protocol_manager_next_actions(
        &json!({
            "has_pending_transfer": true,
            "current_manager_address": "0xmanager",
            "new_manager_address": "0xnew"
        }),
        &args,
        Vec::new(),
    );

    assert_eq!(
        next_actions,
        vec![
            "pcl protocol-manager --project project-1 --nonce --address 0xmanager",
            "pcl protocol-manager --project project-1 --accept-calldata",
        ]
    );
}

#[test]
fn deployment_output_only_redacts_artifacts_for_human_mode() {
    let deployment_data = json!({
        "project": {"project_id": "project-1", "project_name": "Demo"},
        "submitted_assertions": [
            {
                "id": "assertion-1",
                "contract_name": "Guard",
                "source_code": "contract Guard { function ok() external {} }",
                "bytecode": "0x6080604052348015600e575f80fd5b50"
            }
        ],
        "staging_assertions": [],
        "available_contracts": [],
        "_meta": {"sources": ["offchain"]}
    });

    let json_data =
        workflow_data_for_output_mode("deployments", &deployment_data, OutputMode::Json);
    let toon_data =
        workflow_data_for_output_mode("deployments", &deployment_data, OutputMode::Toon);

    assert_eq!(
        json_data["submitted_assertions"][0]["source_code"],
        "contract Guard { function ok() external {} }"
    );
    assert_eq!(
        json_data["submitted_assertions"][0]["bytecode"],
        "0x6080604052348015600e575f80fd5b50"
    );
    assert_eq!(
        toon_data["submitted_assertions"][0]["source_code"],
        "contract Guard { function ok() external {} }"
    );
    assert_eq!(
        toon_data["submitted_assertions"][0]["bytecode"],
        "0x6080604052348015600e575f80fd5b50"
    );

    let compact = workflow_data_for_output_mode("deployments", &deployment_data, OutputMode::Human);
    let rendered = serde_json::to_string(&compact).expect("json render");

    assert!(rendered.contains("\"redacted\":true"), "{rendered}");
    assert!(!rendered.contains("contract Guard"), "{rendered}");
    assert!(!rendered.contains("0x608060405234"), "{rendered}");
    assert_eq!(
        compact["submitted_assertions"][0]["source_code"]["reason"],
        "large_artifact"
    );
    assert_eq!(
        compact["submitted_assertions"][0]["bytecode"]["reason"],
        "large_artifact"
    );
}

#[test]
fn human_output_formats_non_empty_releases_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": [
                {
                    "id": "release-1",
                    "releaseNumber": 2,
                    "environment": "production",
                    "status": "active",
                    "createdAt": "2026-05-18T18:00:00Z"
                }
            ],
            "request": {"method": "GET", "path": "/projects/project-1/releases"},
            "response": {"status": 200, "request_id": "req_releases"},
            "next_actions": ["pcl releases show project-1 release-1"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Releases\n",
            "Release",
            "Environment",
            "Status",
            "release-1",
            "production",
        ],
    );
    assert_output_omits(&output, &["Visibility"]);
}

#[test]
fn human_output_formats_contract_lists_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "data": {
                    "contracts": [
                        {
                            "id": "59144_0xabc",
                            "address": "0xabc",
                            "chain_id": 59144,
                            "manager": "0xmanager",
                            "contract_name": "LineaSettler"
                        }
                    ]
                },
                "_meta": {"sources": ["offchain"], "fetchedAt": "2026-05-18T18:00:00Z"}
            },
            "request": {"method": "GET", "path": "/views/projects/project-1/contracts"},
            "response": {"status": 200, "request_id": "req_contracts"},
            "next_actions": ["pcl contracts --project project-1 --adopter-id 59144_0xabc"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Contracts\n",
            "Contract",
            "Chain",
            "Address",
            "Manager",
            "LineaSettler",
            "0xabc",
        ],
    );
}

#[test]
fn human_output_formats_assertion_lists_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "data": {
                    "assertions": [
                        {
                            "assertion_id": "0xassertion",
                            "contract_name": "AllowanceAssertion",
                            "environment": "PRODUCTION",
                            "lifecycle": "enforced",
                            "deployment_instances": [{ "id": "one" }, { "id": "two" }]
                        }
                    ]
                },
                "_meta": {"sources": ["offchain"], "fetchedAt": "2026-05-18T18:00:00Z"}
            },
            "request": {"method": "GET", "path": "/views/projects/project-1/assertions"},
            "response": {"status": 200, "request_id": "req_assertions"},
            "next_actions": ["pcl assertions --project-id project-1 --assertion-id 0xassertion"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Assertions\n",
            "Contract",
            "Lifecycle",
            "Instances",
            "AllowanceAssertion",
            "enforced",
            "0xassertion",
        ],
    );
}

#[test]
fn human_output_formats_project_details_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "project_id": "project-1",
                "project_name": "Private Test",
                "project_description": null,
                "project_networks": ["59144"],
                "chain_names": ["Linea Mainnet"],
                "created_at": "2026-05-06T16:50:17+00:00",
                "updated_at": "2026-05-06T16:51:17+00:00",
                "is_private": true,
                "is_dev": false,
                "submitted_assertion_ids": [],
                "saved_count": 0,
                "protocol_manager_address": null,
                "slug": "private-test"
            },
            "next_actions": [
                "pcl projects show project-1",
                "pcl assertions --project-id project-1"
            ],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Project\n",
            "ID: project-1",
            "Visibility: private",
            "Networks: Linea Mainnet",
            "Submitted assertions: 0 assertions",
        ],
    );
    assert_output_omits(&output, &["Project Id:", "item(s)"]);
}

#[test]
fn human_output_names_success_only_mutations() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {"success": true},
            "request": {"method": "DELETE", "path": "/projects/project-1"},
            "response": {"status": 200, "request_id": "req_delete"},
            "next_actions": ["pcl projects mine"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(&output, &["Project deleted"]);
    assert_output_omits(&output, &["Success: yes"]);

    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {"success": true},
            "request": {"method": "DELETE", "path": "/projects/project-1/invitations/invite-1"},
            "response": {"status": 200, "request_id": "req_revoke"},
            "next_actions": ["pcl access invitations project-1"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(&output, &["Invitation revoked"]);
    assert_output_omits(&output, &["Success: yes"]);
}

#[test]
fn human_output_formats_project_home_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "data": {
                    "member_projects": [
                        {
                            "project_id": "project-1",
                            "project_name": "Private Test",
                            "slug": "private-test",
                            "chain_names": ["Linea Mainnet"],
                            "is_private": true
                        }
                    ],
                    "saved_projects": [],
                    "no_project_adopters": []
                },
                "_meta": {
                    "sources": ["offchain"],
                    "fetchedAt": "2026-05-10T04:16:00Z"
                }
            },
            "request": {"method": "GET", "path": "/views/projects/home"},
            "response": {"status": 200, "request_id": "req_projects_home"},
            "next_actions": ["pcl projects show project-1"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Your projects\n",
            "Showing 1 project you belong to",
            "Updated: 2026-05-10 04:16",
            "Source: Phylax platform index",
            "Project",
            "Slug",
            "Network",
            "Visibility",
            "Private Test",
            "private-test",
            "Linea Mainnet",
            "private",
            "project-1",
            "Saved projects: 0 projects",
            "Contracts without a project: 0 contracts",
        ],
    );
    assert_output_omits(&output, &["Member projects:", "Details:"]);
}

#[test]
fn human_output_formats_invitations_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "invitations": [
                    {
                        "id": "invite-1",
                        "invitee_identifier": "cli-ux-test@example.invalid",
                        "role": "viewer"
                    }
                ]
            },
            "request": {"method": "GET", "path": "/projects/project-1/invitations"},
            "response": {"status": 200, "request_id": "req_invitations"},
            "next_actions": ["pcl access invite project-1 --body-template"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Invitations\n",
            "Showing 1 invitation",
            "cli-ux-test@example.invalid",
            "viewer",
            "pending",
            "pcl access invite project-1 --body-template",
        ],
    );
    assert_output_omits(&output, &["--body '{...}'"]);
}

#[test]
fn human_output_formats_mixed_search_results_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "projects": [],
                "contracts": [
                    {
                        "data": {
                            "contract_name": "LineaSettler",
                            "network": "59144",
                            "address": "0xc026251dc69f6e3556331b2e14e72eb4a34dd55a",
                            "related_project_slug": "0x-settler"
                        },
                        "foundBy": "contract name"
                    }
                ],
                "assertions": []
            },
            "next_actions": [
                "pcl projects show 0x-settler",
                "pcl contracts --project 0x-settler"
            ],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Search results",
            "Projects: 0",
            "Contracts: 1",
            "LineaSettler",
        ],
    );
    assert_output_omits(&output, &["Assertions\nShowing 0"]);
}

#[test]
fn human_errors_include_api_reason_and_hide_internal_actions() {
    let output = envelope_output_string(
        &json!({
            "status": "error",
            "error": {
                "code": "auth.forbidden",
                "message": "API request failed with status 403 for GET /system-status",
                "request_id": "req_forbidden",
                "http": {
                    "method": "GET",
                    "path": "/system-status",
                    "status": 403,
                    "body": {"error": "System status checks are temporarily disabled"}
                }
            },
            "next_actions": [
                "Read error.http.body for the API-provided reason",
                "Check whether the endpoint is enabled and your user has permission"
            ],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "API reason: System status checks are temporarily disabled",
            "Request ID: req_forbidden",
        ],
    );
    assert_output_omits(&output, &["Code: auth.forbidden", "Read error.http.body"]);
}

#[test]
fn human_cli_errors_strip_raw_usage_dump() {
    let output = envelope_output_string(
        &json!({
            "status": "error",
            "error": {
                "code": "cli.unknown_argument",
                "message": "error: unexpected argument '--limit' found\n\nUsage: pcl api list [OPTIONS]\n\nFor more information, try '--help'.",
                "recoverable": true
            },
            "next_actions": ["pcl --help", "pcl api manifest --toon"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(&output, &["unexpected argument '--limit' found"]);
    assert_output_omits(&output, &["error:", "Usage:", "--toon"]);
}

#[test]
fn human_llms_guide_keeps_toon_commands() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "purpose": "CLI-native control surface.",
                "consumption_order": [
                    "pcl doctor --toon",
                    "pcl auth ensure --toon",
                    "pcl api manifest --toon"
                ]
            },
            "next_actions": ["pcl doctor --toon", "pcl api manifest --toon"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(&output, &["pcl doctor --toon", "pcl api manifest --toon"]);
}

#[test]
fn human_incident_detail_uses_readable_sections() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "data": {
                    "incident_id": "incident-1",
                    "public_reference_id": "ref-1",
                    "assertion_id": "assertion-1",
                    "assertion_adopter_id": "adopter-1",
                    "chain_id": 59144,
                    "window_start": "2026-05-11T17:59:26+00:00",
                    "environment": "production",
                    "assertion": {
                        "assertion_id": "assertion-1",
                        "title": "AllowanceAssertion",
                        "description": "short description"
                    },
                    "assertion_adopter": {
                        "id": "adopter-1",
                        "name": "LineaSettler",
                        "address": "0xc026251dc69f6e3556331b2e14e72eb4a34dd55a"
                    },
                    "invalidating_transactions": [{
                        "id": "tx-row-1",
                        "transaction_hash": "0x8b42e518623666080dcda9fdc5bdd73473834372ccfc8d634d0836f4a55308a1",
                        "incident_timestamp": "2026-05-11T18:17:01+00:00",
                        "landed_on_chain": false,
                        "debug_traces": [{"status": "completed"}]
                    }],
                    "transaction_count": 1,
                    "traces_completed": 1,
                    "traces_pending": 0
                }
            },
            "next_actions": ["pcl incidents --incident-id incident-1 --tx-id 0x8b42"]
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Incident\nID: incident-1",
            "Assertion\nTitle: AllowanceAssertion",
            "Assertion adopter\nName: LineaSettler",
            "Invalidating transactions (first 1 of 1)",
        ],
    );
    assert_output_omits(&output, &["Assertion: Assertion ID="]);
}

#[test]
fn human_output_formats_surface_lists_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "workflows": [
                    {
                        "name": "incident-investigation",
                        "description": "Export incidents and inspect traces.",
                        "steps": [
                            {"command": "pcl doctor --toon", "output": "environment readiness"}
                        ]
                    }
                ]
            },
            "next_actions": ["pcl schema list --toon"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Workflows\n",
            "incident-investigation",
            "Export incidents and inspect traces.",
            "pcl schema list",
        ],
    );
    assert_output_omits(&output, &["--toon", "Details:"]);
}

#[test]
fn human_output_honors_display_metadata_before_shape_detection() {
    let output = human_string(&with_envelope_metadata(json!({
        "status": "ok",
        "data": {
            "_display": {
                "kind": "collection",
                "title": "Projects",
                "collection": "projects",
                "columns": [
                    {"label": "Name", "path": "project_name"},
                    {"label": "ID", "path": "project_id"}
                ],
                "empty": "No projects found."
            },
            "projects": [
                {"project_name": "Demo", "project_id": "project-1", "ignored": "hidden"}
            ]
        },
        "next_actions": []
    })));

    assert_output_contains(&output, &["Projects\n", "Name", "Demo", "project-1"]);
    assert_output_omits(&output, &["ignored"]);
}

#[test]
fn human_output_formats_schema_action_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "workflow": "incidents",
                "action": {
                    "name": "list_public",
                    "auth": false,
                    "method": "GET",
                    "path": "/views/public/incidents",
                    "optional_flags": ["--page", "--limit"],
                    "example": "pcl incidents --limit 5 --toon"
                }
            },
            "next_actions": ["pcl workflows --toon"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Schema: incidents",
            "Action: list_public",
            "Request: GET /views/public/incidents",
            "Example: pcl incidents --limit 5",
        ],
    );
    assert_output_omits(&output, &["--toon"]);
}
#[test]
fn human_output_formats_api_discovery_for_people() {
    let output = envelope_output_string(
        &json!({
            "status": "ok",
            "data": {
                "operations": [
                    {
                        "operation_id": "get_views_public_incidents",
                        "method": "GET",
                        "path": "/views/public/incidents",
                        "raw_api_use": {"policy": "prefer_workflow"}
                    }
                ]
            },
            "next_actions": ["pcl api inspect get_views_public_incidents --toon"],
        }),
        false,
    )
    .unwrap();

    assert_output_contains(
        &output,
        &[
            "Operations\n",
            "GET",
            "/views/public/incidents",
            "Prefer workflow",
        ],
    );
    assert_output_omits(&output, &["--toon"]);
}

#[test]
fn toon_output_round_trips_comma_containing_strings() {
    let value = json!({
        "items": [
            {
                "id": "project-1",
                "name": "Alpha, Beta"
            }
        ]
    });
    let output = toon_string(&value);
    let decoded: Value = toon_format::decode_default(&output).unwrap();

    assert_eq!(decoded, value);
}

#[test]
fn machine_envelopes_keep_required_root_contract() {
    let envelopes = [
        ok_envelope(json!({"healthy": true})),
        template_envelope(body_template("empty_object")),
        ApiCommandError::InvalidPath("health".to_string()).json_envelope(),
    ];

    for envelope in envelopes {
        assert!(envelope["status"].as_str().is_some());
        assert_eq!(envelope["schema_version"], ENVELOPE_SCHEMA_VERSION);
        assert_eq!(envelope["pcl_version"], env!("CARGO_PKG_VERSION"));
        assert!(
            envelope["next_actions"].as_array().is_some(),
            "missing next_actions in {envelope:?}"
        );
        if envelope["status"] == "ok" {
            assert!(
                envelope.get("data").is_some(),
                "missing data in {envelope:?}"
            );
        } else {
            assert!(
                envelope.get("error").is_some(),
                "missing error in {envelope:?}"
            );
        }
    }
}

#[test]
fn variant_body_templates_return_variant_specific_next_actions() {
    let envelope = template_envelope(body_template("protocol_manager_confirm"));

    assert_eq!(
        envelope["next_actions"],
        json!([
            "Choose one entry from data.body_variants and pass only its body with --body-file <path>",
            "Or pass fields from the chosen variant body with --field key=value"
        ])
    );
    assert_eq!(envelope["schema_version"], ENVELOPE_SCHEMA_VERSION);
    assert_eq!(envelope["pcl_version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn manifest_lists_structured_actions_for_every_workflow() {
    let manifest = api_manifest();
    assert_eq!(manifest["default_output"], "human");
    assert_eq!(
        manifest["output_modes"]["toon"],
        "Pass --toon for compact machine-readable envelopes."
    );
    assert!(
        manifest["output_modes"]["json"]
            .as_str()
            .is_some_and(|value| value.contains("--json"))
    );
    let commands = manifest["commands"].as_array().unwrap();
    for command_name in spec::WORKFLOW_SPECS
        .iter()
        .filter(|spec| matches!(spec.layer, spec::WorkflowLayer::Workflow))
        .map(|spec| spec.name)
    {
        let command = commands
            .iter()
            .find(|command| {
                command["command"]
                    .as_str()
                    .is_some_and(|value| value.contains(command_name))
            })
            .unwrap_or_else(|| panic!("missing manifest command {command_name}"));
        assert!(
            command["output"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "missing output shape for {command_name}"
        );
        assert!(
            command["output_policy"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "missing output policy for {command_name}"
        );
        let actions = command["actions"].as_array().unwrap_or_else(|| {
            panic!("missing structured actions for manifest command {command_name}")
        });
        assert!(!actions.is_empty(), "empty actions for {command_name}");
        for action in actions {
            for field in ["name", "method", "path", "example"] {
                assert!(
                    action[field]
                        .as_str()
                        .is_some_and(|value| !value.is_empty()),
                    "missing {field} for {command_name} action {action:?}"
                );
            }
            assert!(
                action["example"]
                    .as_str()
                    .is_some_and(|example| example.contains("--toon")),
                "agent example must include --toon for {command_name} action {action:?}"
            );
            assert!(
                action["auth"].as_bool().is_some(),
                "missing auth for {command_name} action {action:?}"
            );
            assert!(
                matches!(
                    action["method"].as_str(),
                    Some("GET" | "POST" | "PUT" | "PATCH" | "DELETE")
                ),
                "invalid method for {command_name} action {action:?}"
            );
            let path = action["path"].as_str().unwrap();
            if path.contains('{') {
                let required_flags = action["required_flags"].as_array().unwrap_or_else(|| {
                        panic!(
                            "path placeholders require required_flags for {command_name} action {action:?}"
                        )
                    });
                assert!(
                    !required_flags.is_empty(),
                    "empty required_flags for {command_name} action {action:?}"
                );
            }
        }
    }

    for spec in spec::WORKFLOW_SPECS
        .iter()
        .filter(|spec| matches!(spec.layer, spec::WorkflowLayer::Workflow))
        .filter(|spec| !spec.preferred_subcommands.is_empty())
    {
        let manifest_spec = manifest["workflow_specs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == spec.name)
            .unwrap_or_else(|| panic!("missing workflow spec {}", spec.name));
        assert_eq!(
            manifest_spec["preferred_subcommands"],
            json!(spec.preferred_subcommands),
            "manifest should expose preferred subcommands for {}",
            spec.name
        );
        let command = commands
            .iter()
            .find(|command| {
                command["command"]
                    .as_str()
                    .is_some_and(|value| value.contains(spec.name))
            })
            .unwrap();
        let actions = command["actions"].as_array().unwrap();
        assert!(
            actions.iter().all(|action| {
                action["example"].as_str().is_some_and(|example| {
                    example.starts_with(&format!("pcl {}", spec.name))
                        && !example.contains(&format!("{} --", spec.name))
                })
            }),
            "preferred examples for {} should use subcommands",
            spec.name
        );
    }

    for example in manifest["examples"].as_array().unwrap() {
        let example = example.as_str().unwrap();
        assert!(
            example.contains("--toon"),
            "top-level manifest example must include --toon: {example}"
        );
    }

    let incident_actions = commands
        .iter()
        .find(|command| {
            command["command"]
                .as_str()
                .is_some_and(|value| value.contains("incidents"))
        })
        .and_then(|command| command["actions"].as_array())
        .unwrap();
    assert!(
        incident_actions.iter().any(|action| {
            action["name"] == "stats"
                && action["path"] == "/projects/{project_id}/incidents/stats"
                && action["required_flags"] == json!(["--project"])
        }),
        "manifest must include project incident stats workflow"
    );
    for (name, flags) in [
        ("list_project", json!(["--project"])),
        ("detail", json!(["--incident-id"])),
        ("trace", json!(["--incident-id", "--tx-id"])),
        ("retry_trace", json!(["--incident-id", "--tx-id"])),
    ] {
        assert!(
            incident_actions
                .iter()
                .any(|action| action["name"] == name && action["required_flags"] == flags),
            "manifest must include required flags for incident action {name}"
        );
    }

    let call_actions = commands
        .iter()
        .find(|command| {
            command["command"]
                .as_str()
                .is_some_and(|value| value.starts_with("pcl api call "))
        })
        .and_then(|command| command["actions"].as_array())
        .unwrap();
    assert!(
        call_actions.iter().any(|action| {
            action["name"] == "paginate"
                && action["method"] == "GET"
                && action["required_flags"] == json!(["--paginate"])
        }),
        "manifest must include generic raw-call pagination"
    );
    let completion_surface = manifest["product_surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|surface| surface["command"] == "pcl completions <shell>")
        .expect("completion surface");
    assert!(
        completion_surface["description"]
            .as_str()
            .is_some_and(|description| description.contains("raw shell completion scripts"))
    );
    assert!(
        manifest["product_surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|surface| surface["command"] != "pcl completions <shell> --toon"),
        "manifest should not advertise envelope mode as the default completions install path"
    );
}

#[test]
fn workflow_definitions_are_the_manifest_source_of_truth() {
    let manifest = api_manifest();
    let commands = manifest["commands"].as_array().unwrap();
    let workflow_definitions = definitions::workflow_definitions();

    for definition in workflow_definitions {
        let command = commands
            .iter()
            .find(|command| command["command"] == definition.command)
            .unwrap_or_else(|| panic!("missing manifest command {}", definition.command));
        assert_eq!(
            command["output_policy"],
            definition.output_policy.as_str(),
            "manifest should expose output policy from workflow definition {}",
            definition.name
        );
        assert_eq!(
            command["actions"].as_array().unwrap().len(),
            definition.actions.len(),
            "manifest action count should come from workflow definition {}",
            definition.name
        );
    }

    assert_eq!(
        definitions::workflow_output_policy("deployments"),
        definitions::WorkflowOutputPolicy::MachineRawHumanCompactArtifacts
    );
    assert_eq!(
        definitions::workflow_output_policy("projects"),
        definitions::WorkflowOutputPolicy::MachineRaw
    );
}

#[test]
fn parser_rejects_conflicting_workflow_actions() {
    assert!(ApiArgs::try_parse_from(["api", "projects", "--save", "--unsave"]).is_err());
    assert!(ApiArgs::try_parse_from(["api", "projects", "--mine", "--saved"]).is_err());
    assert!(
        ApiArgs::try_parse_from([
            "api",
            "releases",
            "--project",
            "project-1",
            "--deploy",
            "--remove"
        ])
        .is_err()
    );
    assert!(
        ApiArgs::try_parse_from(["api", "transfers", "--transfer-id", "t1", "--reject"]).is_err()
    );
}

#[test]
fn parser_accepts_projects_mine_and_home_aliases() {
    let mine = ApiArgs::try_parse_from(["api", "projects", "--mine"]).unwrap();
    let ApiCommand::Projects(args) = mine.command else {
        panic!("expected projects command");
    };
    assert!(args.mine);

    let home = ApiArgs::try_parse_from(["api", "projects", "--home"]).unwrap();
    let ApiCommand::Projects(args) = home.command else {
        panic!("expected projects command");
    };
    assert!(args.mine);
}

#[test]
fn parser_accepts_search_query_as_positional_term() {
    let parsed = ApiArgs::try_parse_from(["api", "search", "settler"]).unwrap();
    let ApiCommand::Search(args) = parsed.command else {
        panic!("expected search command");
    };
    assert_eq!(args.term.as_deref(), Some("settler"));
}

#[test]
fn parser_allows_body_template_without_routing_ids() {
    assert!(ApiArgs::try_parse_from(["api", "releases", "--deploy", "--body-template"]).is_ok());
    assert!(
        ApiArgs::try_parse_from(["api", "deployments", "--confirm", "--body-template"]).is_ok()
    );
    assert!(
        ApiArgs::try_parse_from(["api", "integrations", "--configure", "--body-template"]).is_ok()
    );
    assert!(
        ApiArgs::try_parse_from([
            "api",
            "protocol-manager",
            "--confirm-transfer",
            "--body-template"
        ])
        .is_ok()
    );
}

#[test]
fn parser_accepts_uppercase_http_methods_for_raw_api_calls() {
    assert!(ApiArgs::try_parse_from(["api", "call", "GET", "/views/public/incidents"]).is_ok());
    assert!(ApiArgs::try_parse_from(["api", "list", "--method", "GET"]).is_ok());
}

#[test]
fn summarizes_openapi_body_fields() {
    let operation = json!({
        "requestBody": {
            "content": {
                "application/json": {
                    "schema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": {"type": "string"},
                            "private": {"type": "boolean"}
                        }
                    }
                }
            }
        }
    });

    assert_eq!(required_body_fields(&operation), vec!["name"]);
    assert_eq!(body_fields(&operation).len(), 2);
    assert_eq!(
        openapi_body_template(&operation),
        json!({"name": "<string>", "private": false})
    );
}

#[test]
fn summarizes_one_of_body_variants() {
    let operation = json!({
        "requestBody": {
            "content": {
                "application/json": {
                    "schema": {
                        "oneOf": [
                            {
                                "type": "object",
                                "required": ["mode", "new_manager_address"],
                                "properties": {
                                    "mode": {"type": "string", "const": "direct"},
                                    "new_manager_address": {"type": "string"}
                                }
                            },
                            {
                                "type": "object",
                                "required": ["mode", "tx_hash", "chain_id", "new_manager_address"],
                                "properties": {
                                    "mode": {"type": "string", "const": "onchain"},
                                    "tx_hash": {"type": "string"},
                                    "chain_id": {"type": "integer"},
                                    "new_manager_address": {"type": "string"}
                                }
                            }
                        ]
                    }
                }
            }
        }
    });

    let variants = body_variants(&operation);
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0]["name"], "direct");
    assert_eq!(
        variants[0]["required_body_fields"],
        json!(["mode", "new_manager_address"])
    );
    assert_eq!(
        variants[0]["body_template"],
        json!({"mode": "direct", "new_manager_address": "<string>"})
    );
    assert_eq!(variants[1]["name"], "onchain");
    assert_eq!(variants[1]["body_fields"].as_array().unwrap().len(), 4);
}

#[test]
fn project_list_next_actions_use_returned_project_id() {
    let data = json!({
        "data": {
            "items": [
                {
                    "project_id": "project-1",
                    "project_name": "Project One"
                }
            ]
        }
    });

    let next_actions = projects_next_actions(&data, Vec::new());
    assert_eq!(
        next_actions,
        vec![
            "pcl projects show project-1",
            "pcl assertions --project-id project-1",
            "pcl incidents --project-id project-1 --limit 10",
        ]
    );
}
