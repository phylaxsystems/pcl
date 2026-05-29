use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        IncidentsArgs,
        WorkflowOperation,
        WorkflowRequest,
    },
    first_string_field,
    push_query,
    required_arg,
    workflow_operation_with_body,
};
use serde_json::Value;

pub(in crate::api) fn incidents_request(
    args: &IncidentsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
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
            return workflow_operation_with_body(
                WorkflowOperation::new(
                    HttpMethod::Post,
                    "post_incidents_incident_id_transactions_invalidating_transaction_id_trace_retry",
                )
                .path_param("incident_id", incident_id)
                .path_param("invalidating_transaction_id", &tx_id),
                true,
                Some("{}".to_string()),
                vec![format!(
                    "pcl incidents --incident-id {incident_id} --tx-id {tx_id}"
                )],
            );
        }
        let request = if let Some(tx_id) = &args.tx_id {
            WorkflowRequest::from_operation(
                WorkflowOperation::new(
                    HttpMethod::Get,
                    "get_views_incidents_incident_id_transactions_invalidating_transaction_id_trace",
                )
                .path_param("incidentId", incident_id)
                .path_param("invalidatingTransactionId", tx_id),
                query,
                None,
                true,
                vec![
                    "pcl incidents --limit 5".to_string(),
                    "pcl api inspect get_views_incidents_incident_id_transactions_invalidating_transaction_id_trace"
                        .to_string(),
                ],
            )?
        } else {
            WorkflowRequest::from_operation(
                WorkflowOperation::new(HttpMethod::Get, "get_views_incidents_incident_id")
                    .path_param("incidentId", incident_id),
                query,
                None,
                true,
                vec![
                    "pcl incidents --limit 5".to_string(),
                    "pcl api inspect get_views_incidents_incident_id".to_string(),
                ],
            )?
        };
        return Ok(request);
    }

    if let Some(project_id) = &args.project_id {
        if args.stats {
            return WorkflowRequest::from_operation(
                WorkflowOperation::new(HttpMethod::Get, "get_projects_project_id_incidents_stats")
                    .path_param("project_id", project_id),
                query,
                None,
                true,
                vec![format!(
                    "pcl incidents --project-id {project_id} --limit 10"
                )],
            );
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
        return WorkflowRequest::from_operation(
            WorkflowOperation::new(HttpMethod::Get, "get_views_projects_project_id_incidents")
                .path_param("projectId", project_id),
            query,
            None,
            true,
            vec![
                format!("pcl assertions --project-id {project_id}"),
                "pcl incidents --limit 5".to_string(),
            ],
        );
    }

    push_query(&mut query, "network", args.network);
    push_query(&mut query, "sort", args.sort.as_deref());
    push_query(&mut query, "devMode", args.dev_mode.as_deref());
    WorkflowRequest::from_operation(
        WorkflowOperation::new(HttpMethod::Get, "get_views_public_incidents"),
        query,
        None,
        false,
        vec![
            "pcl incidents --project-id <project-id> --limit 10".to_string(),
            "pcl projects list --limit 10".to_string(),
        ],
    )
}

pub(in crate::api) fn incidents_next_actions(
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
                    first_string_field(
                        transaction,
                        &[
                            "id",
                            "tx_id",
                            "invalidating_transaction_id",
                            "invalidatingTransactionId",
                        ],
                    )
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
            "pcl projects list --limit 10".to_string(),
        ]
    })
}

workflow_definition!(
    "incidents",
    command: "pcl incidents [--project-id <id>] [--incident-id <id>] [--stats] [--limit <n>] [--all --output <file>]",
    description: "List public incidents, project incidents, fetch all incident pages, inspect incident detail, incident stats, or incident trace.",
    output: "incident data from /views/public/incidents, /views/projects/{projectId}/incidents, /views/incidents/{incidentId}, or /projects/{project_id}/incidents/stats",
    policy: MachineRaw,
    actions: [
        action!("list_public", false, "get_views_public_incidents", "pcl incidents --limit 5", optional: ["--page", "--limit", "--network", "--sort", "--dev-mode", "--all", "--max-pages", "--output"]),
        action!("list_project", true, "get_views_projects_project_id_incidents", "pcl incidents --project <project-ref> --all --limit 50 --output incidents.json", required: ["--project"], optional: ["--page", "--limit", "--assertion-id", "--adopter-id", "--environment", "--from", "--to", "--all", "--max-pages", "--output"]),
        action!("stats", true, "get_projects_project_id_incidents_stats", "pcl incidents --project <project-ref> --stats", required: ["--project"]),
        action!("detail", true, "get_views_incidents_incident_id", "pcl incidents --incident-id <incident-id>", required: ["--incident-id"]),
        action!("trace", true, "get_views_incidents_incident_id_transactions_invalidating_transaction_id_trace", "pcl incidents --incident-id <incident-id> --tx-id <invalidating-transaction-id>", required: ["--incident-id", "--tx-id"]),
        action!("retry_trace", true, "post_incidents_incident_id_transactions_invalidating_transaction_id_trace_retry", "pcl incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace", required: ["--incident-id", "--tx-id"], body_template: "empty_object"),
    ],
);
