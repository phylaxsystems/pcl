use super::*;
use crate::config::UserAuth;
use chrono::{
    TimeZone,
    Utc,
};
use clap::Parser;
use mockito::Matcher;
use std::path::Path;

fn test_request_log_path() -> &'static Path {
    Path::new("/tmp/pcl-test-requests.jsonl")
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
        home: false,
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
        create: false,
        preview: false,
        deploy: false,
        remove: false,
        deploy_calldata: false,
        remove_calldata: false,
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
            "pcl api inspect post_project_widgets --json".to_string(),
            "Use data.example_call after filling placeholders".to_string()
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
        example_call(HttpMethod::Post, "/projects", &json!({"requestBody": {}})),
        "pcl api call post /projects --body '{}'"
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
fn synthesizes_missing_operation_ids() {
    assert_eq!(
        synthetic_operation_id(HttpMethod::Post, "/web/auth/bootstrap-session"),
        "post_web_auth_bootstrap_session"
    );
}

#[test]
fn builds_public_incidents_workflow_request() {
    let request = incidents_request(&IncidentsArgs {
        project_id: None,
        incident_id: None,
        tx_id: None,
        assertion_id: None,
        assertion_adopter_id: None,
        environment: None,
        from_date: None,
        to_date: None,
        page: None,
        limit: Some(5),
        network: None,
        sort: None,
        dev_mode: None,
        stats: false,
        retry_trace: false,
        all: false,
        max_pages: None,
        output: None,
        jsonl: false,
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
        incident_id: None,
        tx_id: None,
        assertion_id: Some("assertion-1".to_string()),
        assertion_adopter_id: None,
        environment: Some("production".to_string()),
        from_date: None,
        to_date: None,
        page: None,
        limit: Some(10),
        network: None,
        sort: None,
        dev_mode: None,
        stats: false,
        retry_trace: false,
        all: false,
        max_pages: None,
        output: None,
        jsonl: false,
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
        stats: true,
        retry_trace: false,
        all: false,
        max_pages: None,
        output: None,
        jsonl: false,
    })
    .unwrap();

    assert_eq!(request.path, "/projects/project-1/incidents/stats");
    assert_eq!(request.method.openapi_key(), "get");
    assert!(request.require_auth);
}

#[test]
fn builds_incident_trace_retry_request() {
    let request = incidents_request(&IncidentsArgs {
        project_id: None,
        incident_id: Some("incident-1".to_string()),
        tx_id: Some("tx-1".to_string()),
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
        retry_trace: true,
        all: false,
        max_pages: None,
        output: None,
        jsonl: false,
    })
    .unwrap();

    assert_eq!(
        request.path,
        "/incidents/incident-1/transactions/tx-1/trace/retry"
    );
    assert_eq!(request.method.openapi_key(), "post");
    assert!(request.require_auth);
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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: server.url().parse().unwrap(),
        allow_unauthenticated: true,
        dry_run: false,
    };
    let request = WorkflowRequest::get("/views/public/incidents", false, Vec::new());

    let data = api
        .call_workflow_paginated(
            &CliConfig::default(),
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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: "https://app.phylax.systems".parse().unwrap(),
        allow_unauthenticated: true,
        dry_run: false,
    };
    let request = WorkflowRequest::get("/views/public/incidents", false, Vec::new());

    let error = api
        .call_workflow_paginated(
            &CliConfig::default(),
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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: server.url().parse().unwrap(),
        allow_unauthenticated: false,
        dry_run: false,
    };
    let config = CliConfig {
        auth: Some(UserAuth {
            access_token: "expired-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            expires_at: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
            user_id: None,
            wallet_address: None,
            email: Some("agent@example.com".to_string()),
        }),
    };

    let output = api
        .run_workflow(
            &config,
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
async fn dry_run_projects_and_assertions_do_not_execute_requests() {
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: "https://app.phylax.systems".parse().unwrap(),
        allow_unauthenticated: false,
        dry_run: true,
    };

    let project_output = api
        .run_projects(
            &CliConfig::default(),
            &ProjectsArgs {
                create: true,
                project_name: Some("Demo".to_string()),
                chain_id: Some(1),
                ..projects_args()
            },
            test_request_log_path(),
        )
        .await
        .unwrap();
    assert_eq!(project_output["status"], "ok");
    assert_eq!(project_output["data"]["dry_run"], true);
    assert_eq!(project_output["data"]["request"]["method"], "POST");
    assert_eq!(project_output["data"]["request"]["path"], "/projects");

    let assertion_output = api
        .run_assertions(
            &CliConfig::default(),
            &AssertionsArgs {
                project_id: Some("project-1".to_string()),
                submit: true,
                body: Some(r#"{"assertions":[]}"#.to_string()),
                ..assertions_args(None)
            },
            test_request_log_path(),
        )
        .await
        .unwrap();
    assert_eq!(assertion_output["status"], "ok");
    assert_eq!(assertion_output["data"]["dry_run"], true);
    assert_eq!(assertion_output["data"]["request"]["method"], "POST");
    assert_eq!(
        assertion_output["data"]["request"]["path"],
        "/projects/project-1/submitted-assertions"
    );
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
    let submitted = assertions_request(&AssertionsArgs {
        submitted: true,
        ..assertions_args(Some("project-1"))
    })
    .unwrap();
    assert_eq!(submitted.path, "/projects/project-1/submitted-assertions");

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
    assert!(error.to_string().contains("--project is required"));
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
    assert_eq!(
        request.query,
        vec![("user_id".to_string(), "user-1".to_string())]
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
        request.query,
        vec![("signerAddress".to_string(), "0xsigner".to_string())]
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
    assert!(project_error.to_string().contains("--project is required"));

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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: server.url().parse().unwrap(),
        allow_unauthenticated: true,
        dry_run: false,
    };
    let config = CliConfig::default();
    let request = WorkflowRequest::get("/health", false, Vec::new());

    let error = api
        .call_workflow_result(&config, &request, test_request_log_path())
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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: server.url().parse().unwrap(),
        allow_unauthenticated: true,
        dry_run: false,
    };
    let request = WorkflowRequest::get("/health", false, vec!["next".to_string()]);

    let envelope = api
        .run_workflow(&CliConfig::default(), request, test_request_log_path())
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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: server.url().parse().unwrap(),
        allow_unauthenticated: true,
        dry_run: false,
    };

    let response = api
        .call_api(
            &CliConfig::default(),
            ApiRequestInput {
                method: HttpMethod::Get,
                path: "/projects/project-1/incidents?environment=production",
                query: &["limit=50".to_string()],
                header: &[],
                body: None,
                body_file: None,
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

#[test]
fn parser_accepts_global_dry_run() {
    let args = ApiArgs::try_parse_from([
        "api",
        "--dry-run",
        "call",
        "post",
        "/web/auth/logout",
        "--body",
        "{}",
    ])
    .unwrap();

    assert!(args.dry_run);
}

#[test]
fn raw_api_dry_run_plans_request_without_auth_or_network() {
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: "https://api.example.com".parse().unwrap(),
        allow_unauthenticated: false,
        dry_run: true,
    };
    let query = vec!["force=true".to_string()];
    let header = vec!["x-test=yes".to_string()];

    let plan = api
        .raw_call_plan(
            ApiRequestInput {
                method: HttpMethod::Post,
                path: "/web/auth/logout?reason=test",
                query: &query,
                header: &header,
                body: Some("{}"),
                body_file: None,
                require_auth: true,
            },
            None,
        )
        .unwrap();

    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["valid"], true);
    assert_eq!(plan["request"]["method"], "POST");
    assert_eq!(plan["request"]["path"], "/web/auth/logout");
    assert_eq!(
        plan["request"]["query"],
        json!([
            {"name": "reason", "value": "test"},
            {"name": "force", "value": "true"}
        ])
    );
    assert_eq!(plan["request"]["body"], json!({}));
    assert_eq!(plan["request"]["auth"]["required"], true);
    assert_eq!(plan["request"]["side_effecting"], true);
    assert_eq!(plan["request"]["destructive"], true);
}

#[test]
fn workflow_dry_run_plans_destructive_requests() {
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: "https://api.example.com".parse().unwrap(),
        allow_unauthenticated: false,
        dry_run: true,
    };
    let request = WorkflowRequest {
        method: HttpMethod::Delete,
        path: "/projects/project-1".to_string(),
        query: vec![("environment".to_string(), "production".to_string())],
        body: None,
        require_auth: true,
        next_actions: Vec::new(),
    };

    let plan = api.workflow_request_plan(&request, None);

    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["valid"], true);
    assert_eq!(plan["request"]["method"], "DELETE");
    assert_eq!(plan["request"]["path"], "/projects/project-1");
    assert_eq!(plan["request"]["auth"]["will_attach_stored_token"], true);
    assert_eq!(plan["request"]["side_effecting"], true);
    assert_eq!(plan["request"]["destructive"], true);
    assert_eq!(plan["request"]["project_resolution"], "not_performed");
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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: server.url().parse().unwrap(),
        allow_unauthenticated: true,
        dry_run: false,
    };

    let response = api
        .call_api_paginated(
            &CliConfig::default(),
            ApiRequestInput {
                method: HttpMethod::Get,
                path: "/views/public/incidents?environment=production",
                query: &[],
                header: &[],
                body: None,
                body_file: None,
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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: server.url().parse().unwrap(),
        allow_unauthenticated: true,
        dry_run: false,
    };

    let response = api
        .call_api_paginated(
            &CliConfig::default(),
            ApiRequestInput {
                method: HttpMethod::Get,
                path: "/custom",
                query: &[],
                header: &[],
                body: None,
                body_file: None,
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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: "https://api.example.com".parse().unwrap(),
        allow_unauthenticated: true,
        dry_run: false,
    };

    let error = api
        .call_api_paginated(
            &CliConfig::default(),
            ApiRequestInput {
                method: HttpMethod::Post,
                path: "/views/public/incidents",
                query: &[],
                header: &[],
                body: None,
                body_file: None,
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
    let api = ApiArgs {
        command: ApiCommand::Manifest,
        api_url: server.url().parse().unwrap(),
        allow_unauthenticated: true,
        dry_run: false,
    };

    let error = api
        .call_api(
            &CliConfig::default(),
            ApiRequestInput {
                method: HttpMethod::Get,
                path: "/health",
                query: &[],
                header: &[],
                body: None,
                body_file: None,
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
fn body_templates_are_action_specific() {
    assert_eq!(
        access_body_template(&AccessArgs {
            project: Some("project-1".to_string()),
            member_user_id: Some("user-1".to_string()),
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
            update_role: true,
            remove: false,
            my_role: false,
            body: None,
            field: Vec::new(),
            body_file: None,
            body_template: true,
        }),
        json!({ "role": "viewer" })
    );
    assert_eq!(
        release_body_template(&ReleasesArgs {
            project: Some("project-1".to_string()),
            release_id: Some("release-1".to_string()),
            signer_address: None,
            create: false,
            preview: false,
            deploy: true,
            remove: false,
            deploy_calldata: false,
            remove_calldata: false,
            body: None,
            field: Vec::new(),
            body_file: None,
            body_template: true,
        }),
        json!({ "chainId": 1, "txHash": "0x..." })
    );
    assert_eq!(
        release_body_template(&ReleasesArgs {
            project: Some("project-1".to_string()),
            release_id: Some("release-1".to_string()),
            signer_address: None,
            create: false,
            preview: false,
            deploy: false,
            remove: false,
            deploy_calldata: true,
            remove_calldata: false,
            body: None,
            field: Vec::new(),
            body_file: None,
            body_template: true,
        }),
        json!({})
    );
    assert_eq!(
        access_body_template(&AccessArgs {
            project: Some("project-1".to_string()),
            member_user_id: None,
            invitation_id: None,
            token: None,
            members: true,
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
            body_template: true,
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
fn default_api_output_is_full_toon_envelope() {
    let output = output_string(
        &json!({
            "status": "ok",
            "data": {"healthy": true},
            "next_actions": ["pcl api list"],
        }),
        false,
    )
    .unwrap();

    assert!(output.contains("status: ok"));
    assert!(output.contains("schema_version: pcl.envelope.v1"));
    assert!(output.contains("pcl_version:"));
    assert!(output.contains("data:"));
    assert!(output.contains("healthy: true"));
    assert!(output.contains("next_actions[1]:"));
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
        dry_run_envelope(json!({
            "dry_run": true,
            "valid": true,
            "request": {"method": "GET", "path": "/health"},
        })),
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
    let commands = manifest["commands"].as_array().unwrap();
    for command_name in [
        "incidents",
        "projects",
        "assertions",
        "search",
        "account",
        "contracts",
        "releases",
        "deployments",
        "access",
        "integrations",
        "protocol-manager",
        "transfers",
        "events",
    ] {
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
}

#[test]
fn parser_rejects_conflicting_workflow_actions() {
    assert!(ApiArgs::try_parse_from(["api", "projects", "--save", "--unsave"]).is_err());
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
            "pcl projects --project-id project-1",
            "pcl assertions --project-id project-1",
            "pcl incidents --project-id project-1 --limit 10",
        ]
    );
}
