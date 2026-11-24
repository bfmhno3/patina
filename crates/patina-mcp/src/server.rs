//! The MCP handler: eight read-only tools, each mapped to one core query.

use std::sync::Arc;

use patina_core::engine::Patina;
use patina_core::query::Query;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};

/// One optional tool argument.
struct Arg {
    name: &'static str,
    kind: &'static str,
    required: bool,
    description: &'static str,
}

/// A tool and the query it runs.
struct ToolDef {
    name: &'static str,
    query: &'static str,
    description: &'static str,
    args: &'static [Arg],
}

const IDENTITY: Arg = Arg {
    name: "identity",
    kind: "string",
    required: false,
    description: "Restrict to one identity (overrides the mode).",
};
const ALL: Arg = Arg {
    name: "all",
    kind: "boolean",
    required: false,
    description: "Ignore the identity mode and show everything.",
};

const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "today",
        query: "today",
        description: "What to do today: habits, follow-ups, deadlines, scheduled and actionable items.",
        args: &[IDENTITY, ALL],
    },
    ToolDef {
        name: "next",
        query: "next",
        description: "The recommended next item, with the reasons for it. Records nothing.",
        args: &[IDENTITY, ALL],
    },
    ToolDef {
        name: "frontier",
        query: "list",
        description: "The actionable frontier: open items with no unmet prerequisites.",
        args: &[IDENTITY],
    },
    ToolDef {
        name: "tree",
        query: "tree",
        description: "The item hierarchy, optionally below one root item.",
        args: &[
            Arg {
                name: "root",
                kind: "string",
                required: false,
                description: "Short id or title of the subtree root.",
            },
            Arg {
                name: "all",
                kind: "boolean",
                required: false,
                description: "Include settled items.",
            },
        ],
    },
    ToolDef {
        name: "show",
        query: "show",
        description: "One item with notes, prerequisites, dependents, and children.",
        args: &[Arg {
            name: "item",
            kind: "string",
            required: true,
            description: "Short id or title of the item.",
        }],
    },
    ToolDef {
        name: "habits",
        query: "habits",
        description: "All habits with today's state and strength.",
        args: &[],
    },
    ToolDef {
        name: "identities",
        query: "identities",
        description: "The visible identities with open item and habit counts.",
        args: &[],
    },
    ToolDef {
        name: "review",
        query: "review",
        description: "The weekly review summary: progress, time, habits, calibration, notes.",
        args: &[],
    },
];

fn tool(def: &ToolDef) -> Tool {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for a in def.args {
        properties.insert(
            a.name.into(),
            json!({"type": a.kind, "description": a.description}),
        );
        if a.required {
            required.push(a.name);
        }
    }
    let mut schema = JsonObject::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    let mut tool = Tool::new(def.name, def.description, Arc::new(schema));
    tool.annotations = Some(ToolAnnotations::new().read_only(true));
    tool
}

/// Serves the read-only tools over one [`Patina`].
pub struct Handler {
    patina: Patina,
}

impl Handler {
    /// Wraps the data root to read.
    pub fn new(patina: Patina) -> Self {
        Self { patina }
    }

    fn run(&self, def: &ToolDef, arguments: Option<JsonObject>) -> CallToolResult {
        let mut args = arguments.unwrap_or_default();
        args.insert("query".into(), json!(def.query));
        let query = match serde_json::from_value::<Query>(Value::Object(args)) {
            Ok(q) => q,
            Err(e) => return failure(e.to_string()),
        };
        match self.patina.query_visible(query, jiff::ZonedDateTime::now()) {
            Ok(out) => match serde_json::to_string_pretty(&out) {
                Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
                Err(e) => failure(e.to_string()),
            },
            Err(e) => failure(e.to_string()),
        }
    }
}

fn failure(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Read-only view of the owner's Patina task system. Identities not listed in [mcp] visible_identities are omitted.",
            )
            .with_server_info(rmcp::model::Implementation::new(
                "patina",
                env!("CARGO_PKG_VERSION"),
            ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            TOOLS.iter().map(tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let def = TOOLS
            .iter()
            .find(|t| t.name == request.name)
            .ok_or_else(|| {
                ErrorData::invalid_params(format!("unknown tool {}", request.name), None)
            })?;
        Ok(self.run(def, request.arguments).into())
    }
}
