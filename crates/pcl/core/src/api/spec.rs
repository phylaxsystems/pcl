use serde_json::{
    Value,
    json,
};

#[derive(Debug)]
pub(super) struct WorkflowSpec {
    pub name: &'static str,
    pub layer: WorkflowLayer,
    pub preferred_subcommands: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub(super) enum WorkflowLayer {
    Workflow,
    Product,
    Raw,
}

impl WorkflowLayer {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Workflow => "workflow",
            Self::Product => "agent_product",
            Self::Raw => "raw_debug_developer",
        }
    }
}

pub(super) const WORKFLOW_SPECS: &[WorkflowSpec] = &[
    WorkflowSpec {
        name: "incidents",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "projects",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[
            "list", "mine", "show", "saved", "create", "update", "delete", "save", "unsave",
            "resolve", "widget",
        ],
    },
    WorkflowSpec {
        name: "assertions",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "search",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "account",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "contracts",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "releases",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[
            "list",
            "show",
            "create",
            "preview",
            "deploy",
            "remove",
            "calldata",
            "backtest-progress",
            "retry-check",
        ],
    },
    WorkflowSpec {
        name: "deployments",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "access",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[
            "members",
            "invitations",
            "pending",
            "preview",
            "accept",
            "invite",
            "resend",
            "revoke",
            "role",
            "member",
            "my-role",
        ],
    },
    WorkflowSpec {
        name: "integrations",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "protocol-manager",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "events",
        layer: WorkflowLayer::Workflow,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "workflows",
        layer: WorkflowLayer::Product,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "schema",
        layer: WorkflowLayer::Product,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "llms",
        layer: WorkflowLayer::Product,
        preferred_subcommands: &[],
    },
    WorkflowSpec {
        name: "api",
        layer: WorkflowLayer::Raw,
        preferred_subcommands: &["list", "inspect", "call", "coverage", "manifest"],
    },
];

pub(super) fn workflow_spec_summary() -> Value {
    Value::Array(
        WORKFLOW_SPECS
            .iter()
            .map(|spec| {
                json!({
                    "name": spec.name,
                    "layer": spec.layer.as_str(),
                    "preferred_subcommands": spec.preferred_subcommands,
                })
            })
            .collect(),
    )
}
