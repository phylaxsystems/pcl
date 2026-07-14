use super::{
    ApiArgs,
    ApiCommand,
    ApiCommandError,
    ApiRequestInput,
    AssertionsArgs,
    ContractsArgs,
    DeploymentsArgs,
    HttpMethod,
    IncidentsArgs,
    PreparedApiRequest,
    ProjectsArgs,
    ProtocolManagerArgs,
    RawPaginationOptions,
    ReleasesArgs,
    SearchArgs,
    WorkflowCallResult,
    WorkflowCommand,
    WorkflowOperation,
    WorkflowPaginationOptions,
    WorkflowRequest,
    access_body_template,
    access_request,
    account_request,
    api_coverage,
    api_manifest,
    assertions_next_actions,
    assertions_request,
    body_template,
    command_next_actions,
    contracts_body_template,
    contracts_next_actions,
    contracts_request,
    deployment_body_template,
    deployments_request,
    events_request,
    extract_paginated_items,
    incidents_next_actions,
    incidents_request,
    inspect_operation,
    integration_body_template,
    integrations_request,
    list_operations,
    next_actions_for_operations,
    ok_envelope,
    openapi_path_matches,
    parse_headers,
    parse_key_values,
    print_output,
    project_body_template,
    projects_next_actions,
    projects_request,
    protocol_manager_body_template,
    protocol_manager_next_actions,
    protocol_manager_request,
    public_raw_call_path,
    query_pairs_value,
    read_api_response,
    release_body_template,
    releases_next_actions,
    releases_request,
    request_body,
    request_id_from_headers,
    response_body_value,
    search_next_actions,
    search_request,
    split_path_and_inline_query,
    template_envelope,
    upsert_query,
    workflow_data_for_output_mode,
    workflow_success_envelope,
    workflow_success_envelope_with_data,
    write_api_coverage_markdown,
    write_json_output_file,
    write_jsonl_items_output_file,
    write_request_log,
};
use crate::{
    auth::refresh_stored_auth,
    config::CliConfig,
    error::AuthError,
};
use dapp_api_client::generated::client::{
    Client as GeneratedClient,
    Error as GeneratedError,
};
use pcl_common::args::CliArgs;
use reqwest::header::{
    HeaderMap,
    HeaderName,
    HeaderValue,
};
use serde_json::{
    Value,
    json,
};
use std::path::Path;

fn generated_error_request_id<E>(error: &GeneratedError<E>) -> Option<String> {
    match error {
        GeneratedError::ErrorResponse(response) => request_id_from_headers(response.headers()),
        GeneratedError::UnexpectedResponse(response) => request_id_from_headers(response.headers()),
        _ => None,
    }
}

fn is_project_path_param(name: &str) -> bool {
    matches!(name, "project_id" | "projectId")
}

async fn generated_error_to_api_error<E>(
    method: &'static str,
    path: &str,
    error: GeneratedError<E>,
) -> ApiCommandError
where
    E: serde::Serialize + std::fmt::Debug,
{
    match error {
        GeneratedError::ErrorResponse(response) => {
            let status = response.status().as_u16();
            let request_id = request_id_from_headers(response.headers());
            let body = serde_json::to_value(response.as_ref()).unwrap_or_else(|error| {
                json!({
                    "error": error.to_string(),
                    "body": format!("{response:?}"),
                })
            });
            ApiCommandError::HttpStatus {
                method,
                path: path.to_string(),
                status,
                request_id,
                body: Box::new(body),
            }
        }
        GeneratedError::UnexpectedResponse(response) => {
            let status = response.status().as_u16();
            let request_id = request_id_from_headers(response.headers());
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(error) => return ApiCommandError::Request(error),
            };
            ApiCommandError::HttpStatus {
                method,
                path: path.to_string(),
                status,
                request_id,
                body: Box::new(response_body_value(&content_type, &bytes)),
            }
        }
        GeneratedError::CommunicationError(error) => ApiCommandError::Request(error),
        GeneratedError::InvalidUpgrade(error) => ApiCommandError::Request(error),
        GeneratedError::ResponseBodyError(error) => ApiCommandError::Request(error),
        GeneratedError::InvalidResponsePayload(bytes, error) => {
            let body = response_body_value("application/json", &bytes);
            ApiCommandError::InvalidWorkflow {
                message: format!("Invalid generated API response payload: {error}; body={body}"),
            }
        }
        GeneratedError::InvalidRequest(message) | GeneratedError::Custom(message) => {
            ApiCommandError::InvalidWorkflow { message }
        }
    }
}

fn print_api_value(output: Value, json_output: bool) -> Result<(), ApiCommandError> {
    print_output(&output, json_output)
}

impl ApiArgs {
    pub(in crate::api) async fn run_workflow_command(
        &self,
        command: &WorkflowCommand,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ApiCommandError> {
        let request_log_path = crate::request_log::request_log_path_for_args(cli_args);
        match command {
            WorkflowCommand::Incidents(args) => {
                print_api_value(
                    self.run_incidents(config, cli_args, args, &request_log_path)
                        .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Projects(args) => {
                print_api_value(
                    self.run_projects(config, cli_args, args, &request_log_path)
                        .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Assertions(args) => {
                print_api_value(
                    self.run_assertions(config, cli_args, args, &request_log_path)
                        .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Search(args) => {
                print_api_value(
                    self.run_search(config, cli_args, args, &request_log_path)
                        .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Account(args) => {
                if args.body_template {
                    return print_api_value(
                        template_envelope(body_template("empty_object")),
                        json_output,
                    );
                }
                print_api_value(
                    self.run_workflow(
                        config,
                        cli_args,
                        "account",
                        account_request(args)?,
                        &request_log_path,
                    )
                    .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Contracts(args) => {
                if args.body_template {
                    return print_api_value(
                        template_envelope(contracts_body_template(args)),
                        json_output,
                    );
                }
                print_api_value(
                    self.run_contracts(config, cli_args, args, &request_log_path)
                        .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Releases(args) => {
                if args.body_template {
                    return print_api_value(
                        template_envelope(release_body_template(args)),
                        json_output,
                    );
                }
                print_api_value(
                    self.run_releases(config, cli_args, args, &request_log_path)
                        .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Deployments(args) => {
                if args.body_template {
                    return print_api_value(
                        template_envelope(deployment_body_template(args)),
                        json_output,
                    );
                }
                print_api_value(
                    self.run_deployments(config, cli_args, args, &request_log_path)
                        .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Access(args) => {
                if args.body_template {
                    return print_api_value(
                        template_envelope(access_body_template(args)),
                        json_output,
                    );
                }
                print_api_value(
                    self.run_workflow(
                        config,
                        cli_args,
                        "access",
                        access_request(args)?,
                        &request_log_path,
                    )
                    .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Integrations(args) => {
                if args.body_template {
                    return print_api_value(
                        template_envelope(integration_body_template(args)),
                        json_output,
                    );
                }
                print_api_value(
                    self.run_workflow(
                        config,
                        cli_args,
                        "integrations",
                        integrations_request(args)?,
                        &request_log_path,
                    )
                    .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::ProtocolManager(args) => {
                if args.body_template {
                    return print_api_value(
                        template_envelope(protocol_manager_body_template(args)),
                        json_output,
                    );
                }
                print_api_value(
                    self.run_protocol_manager(config, cli_args, args, &request_log_path)
                        .await?,
                    json_output,
                )?;
            }
            WorkflowCommand::Events(args) => {
                print_api_value(
                    self.run_workflow(
                        config,
                        cli_args,
                        "events",
                        events_request(args)?,
                        &request_log_path,
                    )
                    .await?,
                    json_output,
                )?;
            }
        }
        Ok(())
    }

    pub async fn run(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ApiCommandError> {
        let request_log_path = crate::request_log::request_log_path_for_args(cli_args);
        match &self.command {
            ApiCommand::Manifest => print_api_value(ok_envelope(api_manifest()), json_output)?,
            ApiCommand::List { filter, method } => {
                let spec = self.fetch_openapi(config).await?;
                let operations = list_operations(&spec, filter.as_deref(), *method)?;
                let next_actions = next_actions_for_operations(&operations);
                print_api_value(
                    json!({
                        "status": "ok",
                        "data": {
                            "operations": operations,
                        },
                        "next_actions": next_actions,
                    }),
                    json_output,
                )?;
            }
            ApiCommand::Inspect {
                operation,
                path,
                full,
            } => {
                let spec = self.fetch_openapi(config).await?;
                let inspected = inspect_operation(&spec, operation, path.as_deref(), *full)?;
                let next_actions = command_next_actions(&inspected);
                print_api_value(
                    json!({
                        "status": "ok",
                        "data": inspected,
                        "next_actions": next_actions,
                    }),
                    json_output,
                )?;
            }
            ApiCommand::Coverage { records, markdown } => {
                let spec = self.fetch_openapi(config).await?;
                let coverage =
                    api_coverage(&spec, &request_log_path, *records, self.api_url.as_str())?;
                if let Some(path) = markdown {
                    write_api_coverage_markdown(path, &coverage)?;
                }
                print_api_value(
                    json!({
                        "status": "ok",
                        "data": coverage,
                        "next_actions": [
                            "pcl requests list --json",
                            "pcl api list --json",
                            "pcl api coverage --markdown api-coverage.md",
                        ],
                    }),
                    json_output,
                )?;
            }
            ApiCommand::Call(args) => {
                if args.jsonl && args.output.is_none() {
                    return Err(ApiCommandError::InvalidWorkflow {
                        message: "--jsonl requires --output".to_string(),
                    });
                }
                let input = ApiRequestInput {
                    method: args.method,
                    path: &args.path,
                    query: &args.query,
                    header: &args.header,
                    body: args.body.as_deref(),
                    body_file: args.body_file.as_ref(),
                    field: &args.field,
                    require_auth: self.raw_call_requires_auth(args.method, &args.path)?,
                };
                let pagination = args.paginate.as_ref().map(|item_field| {
                    RawPaginationOptions {
                        item_field,
                        start_page: args.page.unwrap_or(1),
                        limit: args.limit.unwrap_or(50),
                        page_param: args.page_param.as_deref().unwrap_or("page"),
                        limit_param: args.limit_param.as_deref().unwrap_or("limit"),
                        max_pages: args.max_pages.unwrap_or(100),
                    }
                });
                let (mut response, next_actions) = if let Some(pagination) = pagination {
                    let response = self
                        .call_api_paginated(config, cli_args, input, pagination, &request_log_path)
                        .await?;
                    (
                        response,
                        vec![
                            "Adjust --limit or --max-pages if the result set was truncated"
                                .to_string(),
                            "Use --output results.json to save paginated data".to_string(),
                            "pcl api manifest --json".to_string(),
                        ],
                    )
                } else {
                    let response = self
                        .call_api(config, cli_args, input, &request_log_path)
                        .await?;
                    (
                        response,
                        vec![
                            "pcl api list --json".to_string(),
                            "pcl api manifest --json".to_string(),
                        ],
                    )
                };
                if let Some(path) = &args.output {
                    if args.jsonl {
                        write_jsonl_items_output_file(path, &response)?;
                    } else {
                        let body = response.pointer("/response/body").unwrap_or(&response);
                        write_json_output_file(path, body)?;
                    }
                    if let Some(object) = response.as_object_mut() {
                        object.insert("output_path".to_string(), json!(path.display().to_string()));
                    }
                }
                print_api_value(
                    json!({
                        "status": "ok",
                        "data": response,
                        "next_actions": next_actions,
                    }),
                    json_output,
                )?;
            }
        }

        Ok(())
    }

    pub(in crate::api) async fn call_api_paginated(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        input: ApiRequestInput<'_>,
        pagination: RawPaginationOptions<'_>,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if input.method.openapi_key() != "get" {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--paginate is only supported for GET requests".to_string(),
            });
        }
        if input.body.is_some() || input.body_file.is_some() || !input.field.is_empty() {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--paginate cannot be used with request bodies".to_string(),
            });
        }
        if pagination.limit == 0 {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--limit must be greater than zero".to_string(),
            });
        }
        if pagination.max_pages == 0 {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--max-pages must be greater than zero".to_string(),
            });
        }

        let (path, mut base_query) = split_path_and_inline_query(input.path)?;
        base_query.extend(parse_key_values("query", input.query)?);
        let url = self.api_url(&path)?;
        let headers = parse_headers(input.header)?;
        let operation_id = self.resolve_operation_id(config, input.method, &path).await;
        self.ensure_request_auth(config, cli_args, input.require_auth)
            .await?;
        let client = self.http_client(
            config,
            input.require_auth && !self.allow_unauthenticated,
            input.require_auth && !self.allow_unauthenticated,
        )?;

        let mut items = Vec::new();
        let mut pages_fetched = 0_u64;
        let mut last_page_count = 0_usize;

        for offset in 0..pagination.max_pages {
            let page = pagination.start_page + offset;
            let mut page_query = base_query.clone();
            upsert_query(&mut page_query, pagination.page_param, page.to_string());
            upsert_query(
                &mut page_query,
                pagination.limit_param,
                pagination.limit.to_string(),
            );

            let response = read_api_response(
                client
                    .get(url.clone())
                    .headers(headers.clone())
                    .query(&page_query)
                    .send()
                    .await?,
            )
            .await?;
            write_request_log(
                request_log_path,
                "raw_paginated",
                input.method.as_str(),
                &path,
                response.status.as_u16(),
                response.request_id.as_deref(),
                operation_id.as_deref(),
            );
            if !response.status.is_success() {
                return Err(ApiCommandError::HttpStatus {
                    method: input.method.as_str(),
                    path,
                    status: response.status.as_u16(),
                    request_id: response.request_id,
                    body: Box::new(response.body),
                });
            }

            let page_items =
                extract_paginated_items(&response.body, pagination.item_field).ok_or_else(|| {
                ApiCommandError::InvalidWorkflow {
                    message: format!(
                        "Could not find an array at `{}` or common pagination fields in response",
                        pagination.item_field
                    ),
                }
            })?;
            last_page_count = page_items.len();
            pages_fetched += 1;
            items.extend(page_items);

            if last_page_count < usize::try_from(pagination.limit).unwrap_or(usize::MAX) {
                break;
            }
        }

        let count = items.len();
        Ok(json!({
            "request": {
                "method": input.method.as_str(),
                "path": path,
                "operation_id": operation_id,
                "query": query_pairs_value(&base_query),
                "pagination": {
                    "field": pagination.item_field,
                    "start_page": pagination.start_page,
                    "limit": pagination.limit,
                    "page_param": pagination.page_param,
                    "limit_param": pagination.limit_param,
                    "max_pages": pagination.max_pages,
                }
            },
            "items": items,
            "count": count,
            "pages_fetched": pages_fetched,
            "last_page_count": last_page_count,
        }))
    }

    pub(in crate::api) async fn run_incidents(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &IncidentsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = incidents_request(args)?;
        if args.jsonl && args.output.is_none() {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--jsonl requires --output".to_string(),
            });
        }
        if args.all {
            let mut data = self
                .call_workflow_paginated(
                    config,
                    cli_args,
                    request.clone(),
                    WorkflowPaginationOptions {
                        item_field: "incidents",
                        start_page: args.page.unwrap_or(1),
                        limit: args.limit.unwrap_or(50),
                        max_pages: args.max_pages.unwrap_or(100),
                    },
                    request_log_path,
                )
                .await?;
            if let Some(path) = &args.output {
                if args.jsonl {
                    write_jsonl_items_output_file(path, &data)?;
                } else {
                    write_json_output_file(path, &data)?;
                }
                if let Some(object) = data.as_object_mut() {
                    object.insert("output_path".to_string(), json!(path.display().to_string()));
                }
            }
            let mut next_actions = request.next_actions;
            if args.output.is_none() {
                next_actions.insert(
                    0,
                    "Use --output incidents.json to save large paginated results".to_string(),
                );
            }
            return Ok(json!({
                "status": "ok",
                "data": data,
                "next_actions": next_actions,
            }));
        }
        let result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let next_actions = incidents_next_actions(&result.body, args, request.next_actions);
        Ok(workflow_success_envelope(result, next_actions))
    }

    pub(in crate::api) async fn run_projects(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ProjectsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if args.body_template {
            return Ok(template_envelope(project_body_template(args)));
        }
        let request = projects_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "projects",
            request,
            request_log_path,
            projects_next_actions,
        )
        .await
    }

    pub(in crate::api) async fn run_assertions(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &AssertionsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if args.body_template {
            return Ok(template_envelope(body_template("empty_object")));
        }
        let request = assertions_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "assertions",
            request,
            request_log_path,
            |data, fallback| assertions_next_actions(data, args, fallback),
        )
        .await
    }

    pub(in crate::api) async fn run_search(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &SearchArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = search_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "search",
            request,
            request_log_path,
            search_next_actions,
        )
        .await
    }

    pub(in crate::api) async fn run_contracts(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ContractsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = contracts_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "contracts",
            request,
            request_log_path,
            |data, fallback| contracts_next_actions(data, args, fallback),
        )
        .await
    }

    pub(in crate::api) async fn run_releases(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ReleasesArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = releases_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "releases",
            request,
            request_log_path,
            |data, fallback| releases_next_actions(data, args, fallback),
        )
        .await
    }

    pub(in crate::api) async fn run_deployments(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &DeploymentsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = deployments_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "deployments",
            request,
            request_log_path,
            |_data, fallback| fallback,
        )
        .await
    }

    pub(in crate::api) async fn run_protocol_manager(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ProtocolManagerArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = protocol_manager_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "protocol-manager",
            request,
            request_log_path,
            |data, fallback| protocol_manager_next_actions(data, args, fallback),
        )
        .await
    }

    pub(in crate::api) async fn run_workflow(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        workflow: &'static str,
        request: WorkflowRequest,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        self.run_prepared_workflow(
            config,
            cli_args,
            workflow,
            request,
            request_log_path,
            |_data, fallback| fallback,
        )
        .await
    }

    pub(in crate::api) async fn run_prepared_workflow<F>(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        workflow: &'static str,
        request: WorkflowRequest,
        request_log_path: &Path,
        next_actions_for: F,
    ) -> Result<Value, ApiCommandError>
    where
        F: FnOnce(&Value, Vec<String>) -> Vec<String>,
    {
        let result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let next_actions = next_actions_for(&result.body, request.next_actions);
        let data = workflow_data_for_output_mode(workflow, &result.body, cli_args.output_mode());
        Ok(workflow_success_envelope_with_data(
            result,
            data,
            next_actions,
        ))
    }

    pub(in crate::api) fn auth_plan(
        &self,
        require_auth: bool,
        attach_auth: bool,
        config: &CliConfig,
    ) -> Value {
        let now = chrono::Utc::now();
        let stored_token_present = config
            .auth
            .as_ref()
            .is_some_and(|auth| !auth.access_token.trim().is_empty());
        let stored_token_valid = config
            .auth
            .as_ref()
            .is_some_and(|auth| !auth.access_token.trim().is_empty() && auth.expires_at > now);
        let will_attach_stored_token =
            attach_auth && !self.allow_unauthenticated && stored_token_valid;
        json!({
            "required": require_auth,
            "will_attach_stored_token": will_attach_stored_token,
            "stored_token_present": stored_token_present,
            "stored_token_valid": stored_token_valid,
            "allow_unauthenticated": self.allow_unauthenticated,
        })
    }

    pub(in crate::api) fn raw_call_requires_auth(
        &self,
        method: HttpMethod,
        path: &str,
    ) -> Result<bool, ApiCommandError> {
        if self.allow_unauthenticated {
            return Ok(false);
        }
        let (path, _) = split_path_and_inline_query(path)?;
        Ok(!public_raw_call_path(method, &path))
    }

    pub(in crate::api) async fn fetch_openapi(
        &self,
        config: &CliConfig,
    ) -> Result<Value, ApiCommandError> {
        let client = self.generated_client(config, false, false)?;
        match client.get_openapi().await {
            Ok(response) => Ok(response.into_inner()),
            Err(error) => Err(generated_error_to_api_error("GET", "/openapi", error).await),
        }
    }

    pub(in crate::api) async fn try_refresh_after_401(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
    ) -> Result<bool, ApiCommandError> {
        if !self.refresh_after_401.get() {
            return Ok(false);
        }

        match refresh_stored_auth(config, &self.api_url, cli_args, true).await {
            Ok(_) => Ok(true),
            Err(AuthError::RefreshEndpointNotFound { .. }) => {
                self.refresh_after_401.set(false);
                Ok(false)
            }
            Err(error) => Err(ApiCommandError::AuthRefresh(error)),
        }
    }

    pub(in crate::api) async fn call_api(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        input: ApiRequestInput<'_>,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let (path, mut query) = split_path_and_inline_query(input.path)?;
        query.extend(parse_key_values("query", input.query)?);
        let url = self.api_url(&path)?;
        let headers = parse_headers(input.header)?;
        let body = request_body(input.body, input.body_file, input.field)?;
        let operation_id = self.resolve_operation_id(config, input.method, &path).await;
        let requires_auth = input.require_auth && !self.allow_unauthenticated;
        self.ensure_request_auth(config, cli_args, input.require_auth)
            .await?;

        let json_body = body
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()?;
        let mut response = read_api_response(
            self.send_api_request(
                config,
                PreparedApiRequest {
                    attach_auth: requires_auth,
                    method: input.method,
                    url: &url,
                    headers: &headers,
                    query: &query,
                    body: json_body.as_ref(),
                },
            )
            .await?,
        )
        .await?;
        write_request_log(
            request_log_path,
            "raw",
            input.method.as_str(),
            &path,
            response.status.as_u16(),
            response.request_id.as_deref(),
            operation_id.as_deref(),
        );
        if response.status.as_u16() == 401
            && requires_auth
            && self.try_refresh_after_401(config, cli_args).await?
        {
            response = read_api_response(
                self.send_api_request(
                    config,
                    PreparedApiRequest {
                        attach_auth: requires_auth,
                        method: input.method,
                        url: &url,
                        headers: &headers,
                        query: &query,
                        body: json_body.as_ref(),
                    },
                )
                .await?,
            )
            .await?;
            write_request_log(
                request_log_path,
                "raw_retry_after_refresh",
                input.method.as_str(),
                &path,
                response.status.as_u16(),
                response.request_id.as_deref(),
                operation_id.as_deref(),
            );
            if !response.status.is_success() {
                return Err(ApiCommandError::HttpStatus {
                    method: input.method.as_str(),
                    path,
                    status: response.status.as_u16(),
                    request_id: response.request_id,
                    body: Box::new(response.body),
                });
            }
            return Ok(json!({
                "request": {
                    "method": input.method.as_str(),
                    "path": path,
                    "operation_id": operation_id,
                    "query": query_pairs_value(&query),
                    "retried_after_refresh": true,
                },
                "response": {
                    "status": response.status.as_u16(),
                    "success": response.status.is_success(),
                    "request_id": response.request_id,
                    "headers": response.headers,
                    "body": response.body,
                }
            }));
        }
        if !response.status.is_success() {
            return Err(ApiCommandError::HttpStatus {
                method: input.method.as_str(),
                path,
                status: response.status.as_u16(),
                request_id: response.request_id,
                body: Box::new(response.body),
            });
        }

        Ok(json!({
            "request": {
                "method": input.method.as_str(),
                "path": path,
                "operation_id": operation_id,
                "query": query_pairs_value(&query),
            },
            "response": {
                "status": response.status.as_u16(),
                "success": response.status.is_success(),
                "request_id": response.request_id,
                "headers": response.headers,
                "body": response.body,
            }
        }))
    }

    pub(in crate::api) async fn call_workflow_result(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        request: &WorkflowRequest,
        request_log_path: &Path,
    ) -> Result<WorkflowCallResult, ApiCommandError> {
        let requires_auth = request.require_auth && !self.allow_unauthenticated;
        self.ensure_request_auth(config, cli_args, request.require_auth)
            .await?;
        let attach_auth = self.workflow_attach_auth(request, config);
        let resolved_request = self
            .resolve_workflow_path_params(
                config,
                request,
                attach_auth,
                requires_auth,
                request_log_path,
            )
            .await?;
        let path = resolved_request.path.clone();
        let url = self.api_url(&path)?;
        let json_body = if let Some(body) = &request.body {
            Some(
                self.normalize_request_body(
                    config,
                    request.operation_id,
                    body,
                    attach_auth,
                    requires_auth,
                    request_log_path,
                )
                .await?,
            )
        } else {
            None
        };
        let mut response = read_api_response(
            self.send_workflow_request(config, &resolved_request, &url, json_body.as_ref())
                .await?,
        )
        .await?;
        write_request_log(
            request_log_path,
            "workflow",
            request.method.as_str(),
            &path,
            response.status.as_u16(),
            response.request_id.as_deref(),
            Some(request.operation_id),
        );
        let mut retried_after_refresh = false;
        if response.status.as_u16() == 401
            && requires_auth
            && self.try_refresh_after_401(config, cli_args).await?
        {
            response = read_api_response(
                self.send_workflow_request(config, &resolved_request, &url, json_body.as_ref())
                    .await?,
            )
            .await?;
            retried_after_refresh = true;
            write_request_log(
                request_log_path,
                "workflow_retry_after_refresh",
                request.method.as_str(),
                &path,
                response.status.as_u16(),
                response.request_id.as_deref(),
                Some(request.operation_id),
            );
        }
        if !response.status.is_success() {
            return Err(ApiCommandError::HttpStatus {
                method: request.method.as_str(),
                path,
                status: response.status.as_u16(),
                request_id: response.request_id,
                body: Box::new(response.body),
            });
        }
        let status = response.status;
        let request_id = response.request_id;
        Ok(WorkflowCallResult {
            body: response.body,
            request: json!({
                "method": request.method.as_str(),
                "operation_id": request.operation_id,
                "path": path,
                "query": query_pairs_value(&request.query),
                "auth": self.auth_plan(request.require_auth, request.attach_auth, config),
                "side_effecting": request.method != HttpMethod::Get,
                "retried_after_refresh": retried_after_refresh,
            }),
            response: json!({
                "status": status.as_u16(),
                "success": true,
                "request_id": request_id,
                "fetched_at": chrono::Utc::now().to_rfc3339(),
            }),
        })
    }

    pub(in crate::api) async fn call_workflow_paginated(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        request: WorkflowRequest,
        pagination: WorkflowPaginationOptions<'_>,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if request.method.openapi_key() != "get" {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--all is only supported for GET list workflows".to_string(),
            });
        }
        if pagination.limit == 0 {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--limit must be greater than zero".to_string(),
            });
        }
        if pagination.max_pages == 0 {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--max-pages must be greater than zero".to_string(),
            });
        }

        let mut items = Vec::new();
        let mut pages_fetched = 0_u64;
        let mut last_page_count = 0_usize;

        for offset in 0..pagination.max_pages {
            let page = pagination.start_page + offset;
            let mut page_request = request.clone();
            upsert_query(&mut page_request.query, "page", page.to_string());
            upsert_query(
                &mut page_request.query,
                "limit",
                pagination.limit.to_string(),
            );
            let data = self
                .call_workflow_result(config, cli_args, &page_request, request_log_path)
                .await?
                .body;
            let page_items =
                extract_paginated_items(&data, pagination.item_field).ok_or_else(|| {
                ApiCommandError::InvalidWorkflow {
                    message: format!(
                        "Could not find an array at `{}` or common pagination fields in response",
                        pagination.item_field
                    ),
                }
            })?;
            last_page_count = page_items.len();
            pages_fetched += 1;
            items.extend(page_items);

            if last_page_count < usize::try_from(pagination.limit).unwrap_or(usize::MAX) {
                break;
            }
        }

        let count = items.len();
        Ok(json!({
            "items": items,
            "count": count,
            "pages_fetched": pages_fetched,
            "start_page": pagination.start_page,
            "limit": pagination.limit,
            "max_pages": pagination.max_pages,
            "last_page_count": last_page_count,
        }))
    }

    pub(in crate::api) async fn normalize_request_body(
        &self,
        config: &CliConfig,
        operation_id: &str,
        body: &str,
        attach_auth: bool,
        require_auth: bool,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let mut json_body: Value = serde_json::from_str(body)?;
        if operation_id == "post_projects_saved"
            && let Some(project_ref) = json_body.get("project_id").and_then(Value::as_str)
            && project_ref.parse::<uuid::Uuid>().is_err()
        {
            let project_id = self
                .resolve_project_id(
                    config,
                    project_ref,
                    attach_auth,
                    require_auth,
                    request_log_path,
                )
                .await?;
            if let Some(object) = json_body.as_object_mut() {
                object.insert("project_id".to_string(), Value::String(project_id));
            }
        }
        Ok(json_body)
    }

    pub(in crate::api) async fn resolve_workflow_path_params(
        &self,
        config: &CliConfig,
        request: &WorkflowRequest,
        attach_auth: bool,
        require_auth: bool,
        request_log_path: &Path,
    ) -> Result<WorkflowRequest, ApiCommandError> {
        let mut resolved = request.clone();
        let project_params = request
            .operation
            .path_params()
            .filter(|(name, value)| {
                is_project_path_param(name) && value.parse::<uuid::Uuid>().is_err()
            })
            .map(|(name, value)| (name, value.to_string()))
            .collect::<Vec<_>>();

        for (name, project_ref) in project_params {
            let project_id = self
                .resolve_project_id(
                    config,
                    &project_ref,
                    attach_auth,
                    require_auth,
                    request_log_path,
                )
                .await?;
            resolved.operation.replace_path_param(name, project_id);
        }

        resolved.path = resolved.operation.path()?;
        Ok(resolved)
    }

    pub(in crate::api) async fn resolve_project_id(
        &self,
        config: &CliConfig,
        project_ref: &str,
        attach_auth: bool,
        require_auth: bool,
        request_log_path: &Path,
    ) -> Result<String, ApiCommandError> {
        let operation = WorkflowOperation::new(HttpMethod::Get, "get_projects_resolve_project_ref")
            .path_param("project_ref", project_ref);
        let path = operation.path()?;
        let client = self.generated_client(config, attach_auth, require_auth)?;
        match client.get_projects_resolve_project_ref(project_ref).await {
            Ok(response) => {
                let request_id = request_id_from_headers(response.headers());
                write_request_log(
                    request_log_path,
                    "workflow_project_resolution",
                    "GET",
                    &path,
                    response.status().as_u16(),
                    request_id.as_deref(),
                    Some(operation.operation_id),
                );
                let project_id = response.into_inner().project_id;
                if project_id.is_empty() {
                    Err(ApiCommandError::InvalidWorkflow {
                        message: format!("Could not resolve project reference `{project_ref}`"),
                    })
                } else {
                    Ok(project_id)
                }
            }
            Err(error) => {
                let status = error.status().map(|status| status.as_u16());
                let request_id = generated_error_request_id(&error);
                if let Some(status) = status {
                    write_request_log(
                        request_log_path,
                        "workflow_project_resolution",
                        "GET",
                        &path,
                        status,
                        request_id.as_deref(),
                        Some(operation.operation_id),
                    );
                }
                Err(generated_error_to_api_error("GET", &path, error).await)
            }
        }
    }

    pub(in crate::api) async fn ensure_request_auth(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        require_auth: bool,
    ) -> Result<(), ApiCommandError> {
        if self.allow_unauthenticated || !require_auth {
            return Ok(());
        }
        let Some(auth) = &config.auth else {
            return Err(ApiCommandError::NoAuthToken);
        };
        let now = chrono::Utc::now();
        let seconds_remaining = (auth.expires_at - now).num_seconds();
        if auth.expires_at <= now || seconds_remaining <= crate::config::AUTH_EXPIRES_SOON_SECONDS {
            refresh_stored_auth(config, &self.api_url, cli_args, false)
                .await
                .map_err(ApiCommandError::AuthRefresh)?;
        }
        Ok(())
    }

    pub(in crate::api) async fn send_api_request(
        &self,
        config: &CliConfig,
        request: PreparedApiRequest<'_>,
    ) -> Result<reqwest::Response, ApiCommandError> {
        let client = self.http_client(config, request.attach_auth, request.attach_auth)?;
        let mut builder = client
            .request(request.method.reqwest(), request.url.clone())
            .headers(request.headers.clone());
        if !request.query.is_empty() {
            builder = builder.query(request.query);
        }
        if let Some(body) = request.body {
            builder = builder.json(body);
        }
        Ok(builder.send().await?)
    }

    pub(in crate::api) async fn send_workflow_request(
        &self,
        config: &CliConfig,
        request: &WorkflowRequest,
        url: &url::Url,
        body: Option<&Value>,
    ) -> Result<reqwest::Response, ApiCommandError> {
        let requires_auth = request.require_auth && !self.allow_unauthenticated;
        let attach_auth = self.workflow_attach_auth(request, config);
        let client = self.http_client(config, attach_auth, requires_auth)?;
        let mut builder = client.request(request.method.reqwest(), url.clone());
        if !request.query.is_empty() {
            builder = builder.query(&request.query);
        }
        if let Some(body) = body {
            builder = builder.json(body);
        }
        Ok(builder.send().await?)
    }

    pub(in crate::api) fn workflow_attach_auth(
        &self,
        request: &WorkflowRequest,
        config: &CliConfig,
    ) -> bool {
        if self.allow_unauthenticated {
            return false;
        }
        if request.require_auth {
            return true;
        }
        request.attach_auth
            && config.auth.as_ref().is_some_and(|auth| {
                !auth.access_token.trim().is_empty() && auth.expires_at > chrono::Utc::now()
            })
    }

    pub(in crate::api) fn http_client(
        &self,
        config: &CliConfig,
        attach_auth: bool,
        require_auth: bool,
    ) -> Result<reqwest::Client, ApiCommandError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("api-version"),
            HeaderValue::from_static("1"),
        );

        if attach_auth && let Some(auth) = &config.auth {
            if auth.expires_at <= chrono::Utc::now() {
                return Err(ApiCommandError::ExpiredAuthToken(auth.expires_at));
            }

            let value = format!("Bearer {}", auth.access_token);
            let value = HeaderValue::from_str(&value).map_err(|source| {
                ApiCommandError::InvalidHeaderValue {
                    name: "authorization".to_string(),
                    source,
                }
            })?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
        } else if require_auth {
            return Err(ApiCommandError::NoAuthToken);
        }

        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(ApiCommandError::Request)
    }

    pub(in crate::api) fn generated_client(
        &self,
        config: &CliConfig,
        attach_auth: bool,
        require_auth: bool,
    ) -> Result<GeneratedClient, ApiCommandError> {
        let mut base = self.api_url.clone();
        base.set_path("/api/v1");
        let http_client = self.http_client(config, attach_auth, require_auth)?;
        Ok(GeneratedClient::new_with_client(base.as_str(), http_client))
    }

    pub(in crate::api) async fn resolve_operation_id(
        &self,
        config: &CliConfig,
        method: HttpMethod,
        path: &str,
    ) -> Option<String> {
        let spec = self.fetch_openapi(config).await.ok()?;
        let operations = list_operations(&spec, None, Some(method)).ok()?;
        operations
            .into_iter()
            .find(|operation| openapi_path_matches(&operation.path, path))
            .map(|operation| operation.operation_id)
    }

    pub(in crate::api) fn api_url(&self, path: &str) -> Result<url::Url, ApiCommandError> {
        if !path.starts_with('/') {
            return Err(ApiCommandError::InvalidPath(path.to_string()));
        }

        let mut url = self.api_url.clone();
        url.set_path(&format!("/api/v1{path}"));
        Ok(url)
    }
}
