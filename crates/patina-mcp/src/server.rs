//! The MCP handler: read tools mapped to core queries, plus the write tools the client's
//! configured level allows (`[mcp]` in `config.toml`).

use std::sync::Arc;

use patina_core::access::{self, McpLevel};
use patina_core::command::{self, Command};
use patina_core::engine::Patina;
use patina_core::event::Source;
use patina_core::proposal::ProposalOp;
use patina_core::query::Query;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};

/// One tool argument.
struct Arg {
    name: &'static str,
    kind: &'static str,
    required: bool,
    description: &'static str,
}

/// What a tool does.
enum ToolAction {
    /// Runs the core query with this key.
    Query(&'static str),
    /// Captures `text` into the inbox.
    Capture,
    /// Submits a proposal.
    Propose,
    /// The proposals this client submitted.
    Proposals,
    /// Runs any command.
    Run,
}

/// A tool, the action behind it, and the level it needs.
struct ToolDef {
    name: &'static str,
    action: ToolAction,
    min: McpLevel,
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

const PROPOSE_DESCRIPTION: &str = "Submit a proposal: a batch of plan changes the owner reviews and accepts, in whole or in part. \
`ops` is a list of steps, each an object with an `op` of add_item, edit, note, link, unlink, or move. \
add_item takes a unique `key` (letters, digits, `_`, `-`, up to 32 characters) and an `item` (title, identity, parent, tags, defer, scheduled, deadline, waiting_for, follow_up, important, size, repeat); \
edit takes `item` plus the fields to change; note takes `item` and `text`; link and unlink take `item` and `depends_on`; move takes `item` and `parent`. \
Wherever a step names an item, use its short id or title, or `@key` for an item an earlier add_item step creates. \
Only items you can see resolve. \
Example, splitting \"Write report\" into two linked steps: \
{\"summary\":\"Split the report\",\"ops\":[\
{\"op\":\"add_item\",\"key\":\"outline\",\"item\":{\"title\":\"Outline\",\"parent\":\"Write report\"}},\
{\"op\":\"add_item\",\"key\":\"draft\",\"item\":{\"title\":\"Draft\",\"parent\":\"Write report\"}},\
{\"op\":\"link\",\"item\":\"@draft\",\"depends_on\":\"@outline\"}]}";

const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "today",
        action: ToolAction::Query("today"),
        min: McpLevel::ReadOnly,
        description: "What to do today: habits, follow-ups, deadlines, scheduled and actionable items.",
        args: &[IDENTITY, ALL],
    },
    ToolDef {
        name: "next",
        action: ToolAction::Query("next"),
        min: McpLevel::ReadOnly,
        description: "The recommended next item, with the reasons for it. Records nothing.",
        args: &[IDENTITY, ALL],
    },
    ToolDef {
        name: "frontier",
        action: ToolAction::Query("list"),
        min: McpLevel::ReadOnly,
        description: "The actionable frontier: open items with no unmet prerequisites.",
        args: &[IDENTITY],
    },
    ToolDef {
        name: "tree",
        action: ToolAction::Query("tree"),
        min: McpLevel::ReadOnly,
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
        action: ToolAction::Query("show"),
        min: McpLevel::ReadOnly,
        description: "One item with notes, prerequisites, dependents, and children.",
        args: &[Arg {
            name: "item",
            kind: "string",
            required: true,
            description: "Short id or title of the item.",
        }],
    },
    ToolDef {
        name: "graph",
        action: ToolAction::Query("graph"),
        min: McpLevel::ReadOnly,
        description: "The children of a composite item as a layered dependency graph (layer, frontier flag, edges).",
        args: &[Arg {
            name: "item",
            kind: "string",
            required: true,
            description: "Short id or title of a composite item.",
        }],
    },
    ToolDef {
        name: "habits",
        action: ToolAction::Query("habits"),
        min: McpLevel::ReadOnly,
        description: "All habits with today's state and strength.",
        args: &[],
    },
    ToolDef {
        name: "identities",
        action: ToolAction::Query("identities"),
        min: McpLevel::ReadOnly,
        description: "The visible identities with open item and habit counts.",
        args: &[],
    },
    ToolDef {
        name: "review",
        action: ToolAction::Query("review"),
        min: McpLevel::ReadOnly,
        description: "The weekly review summary: progress, time, habits, calibration, notes.",
        args: &[],
    },
    ToolDef {
        name: "capture",
        action: ToolAction::Capture,
        min: McpLevel::Propose,
        description: "Capture a thought into the owner's inbox for later triage.",
        args: &[Arg {
            name: "text",
            kind: "string",
            required: true,
            description: "The thought to capture.",
        }],
    },
    ToolDef {
        name: "propose",
        action: ToolAction::Propose,
        min: McpLevel::Propose,
        description: PROPOSE_DESCRIPTION,
        args: &[
            Arg {
                name: "summary",
                kind: "string",
                required: true,
                description: "One line saying what the proposal does.",
            },
            Arg {
                name: "ops",
                kind: "array",
                required: true,
                description: "The steps, in order.",
            },
        ],
    },
    ToolDef {
        name: "proposals",
        action: ToolAction::Proposals,
        min: McpLevel::Propose,
        description: "The proposals this client submitted, with the state of each part.",
        args: &[Arg {
            name: "all",
            kind: "boolean",
            required: false,
            description: "Include accepted and rejected proposals.",
        }],
    },
    ToolDef {
        name: "run",
        action: ToolAction::Run,
        min: McpLevel::Trusted,
        description: "Run any write command directly, for example {\"command\":\"complete\",\"item\":\"Write report\"}. The owner can undo everything this client did in bulk.",
        args: &[Arg {
            name: "command",
            kind: "object",
            required: true,
            description: "A command object with a `command` key, as in `patina-core`'s Command type.",
        }],
    },
];

fn level_name(level: McpLevel) -> &'static str {
    match level {
        McpLevel::ReadOnly => "read_only",
        McpLevel::Propose => "propose",
        McpLevel::Trusted => "trusted",
    }
}

fn tool(def: &ToolDef) -> Tool {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for a in def.args {
        let mut schema = json!({"type": a.kind, "description": a.description});
        if a.kind == "array" {
            schema["items"] = json!({"type": "object"});
        }
        properties.insert(a.name.into(), schema);
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
    tool.annotations = Some(if def.min == McpLevel::ReadOnly {
        ToolAnnotations::new().read_only(true)
    } else {
        ToolAnnotations::new()
            .read_only(false)
            .destructive(matches!(def.action, ToolAction::Run))
    });
    tool
}

/// Serves the tools over one [`Patina`].
pub struct Handler {
    patina: Patina,
}

impl Handler {
    /// Wraps the data root to serve.
    pub fn new(patina: Patina) -> Self {
        Self { patina }
    }

    /// The level of `client`, or the settings error text.
    fn level(&self, client: &str) -> Result<McpLevel, String> {
        let settings = self.patina.settings().map_err(|e| e.to_string())?;
        let source = Source::Mcp {
            client: client.to_string(),
        };
        Ok(access::access(&source, &settings.mcp).into())
    }

    fn query(&self, query: Query) -> CallToolResult {
        match self.patina.query_visible(query, jiff::ZonedDateTime::now()) {
            Ok(out) => match serde_json::to_string_pretty(&out) {
                Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
                Err(e) => failure(e.to_string()),
            },
            Err(e) => failure(e.to_string()),
        }
    }

    fn write(&self, cmd: Command, client: &str) -> CallToolResult {
        let source = Source::Mcp {
            client: client.to_string(),
        };
        match self.patina.run(cmd, jiff::ZonedDateTime::now(), source) {
            Ok(out) => match serde_json::to_string_pretty(&out) {
                Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
                Err(e) => failure(e.to_string()),
            },
            Err(e) => failure(e.to_string()),
        }
    }

    fn run(&self, def: &ToolDef, arguments: Option<JsonObject>, client: &str) -> CallToolResult {
        let mut args = arguments.unwrap_or_default();
        let text_arg = |args: &JsonObject, key: &str| -> Result<String, String> {
            args.get(key)
                .and_then(Value::as_str)
                .map(String::from)
                .ok_or_else(|| format!("missing string argument {key}"))
        };
        match &def.action {
            ToolAction::Query(key) => {
                args.insert("query".into(), json!(key));
                match serde_json::from_value::<Query>(Value::Object(args)) {
                    Ok(q) => self.query(q),
                    Err(e) => failure(e.to_string()),
                }
            }
            ToolAction::Capture => match text_arg(&args, "text") {
                Ok(text) => self.write(Command::Capture(command::Capture { text }), client),
                Err(e) => failure(e),
            },
            ToolAction::Propose => {
                let summary = match text_arg(&args, "summary") {
                    Ok(s) => s,
                    Err(e) => return failure(e),
                };
                let ops = match serde_json::from_value::<Vec<ProposalOp>>(
                    args.get("ops").cloned().unwrap_or(Value::Null),
                ) {
                    Ok(ops) => ops,
                    Err(e) => return failure(format!("invalid ops: {e}")),
                };
                self.write(Command::Propose(command::Propose { summary, ops }), client)
            }
            ToolAction::Proposals => {
                let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
                // Whatever `source` the client sends is ignored: it only sees its own.
                self.query(Query::Proposals {
                    all,
                    source: Some(format!("mcp:{client}")),
                })
            }
            ToolAction::Run => {
                match serde_json::from_value::<Command>(
                    args.get("command").cloned().unwrap_or(Value::Null),
                ) {
                    Ok(cmd) => self.write(cmd, client),
                    Err(e) => failure(format!("invalid command: {e}")),
                }
            }
        }
    }
}

fn failure(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

/// The client's name from the handshake; `unknown` before it arrives.
fn client_name(context: &RequestContext<RoleServer>) -> String {
    context
        .peer
        .peer_info()
        .map(|i| i.client_info.name.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "unknown".into())
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Patina task system. Reads hide identities outside [mcp] visible_identities. \
What this client may write depends on its level in [mcp]: read_only, propose (capture to the inbox and submit proposals the owner reviews), or trusted (direct writes the owner can undo in bulk).",
            )
            .with_server_info(rmcp::model::Implementation::new(
                "patina",
                env!("CARGO_PKG_VERSION"),
            ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // A settings error leaves only the read tools; calls then report the error.
        let level = self
            .level(&client_name(&context))
            .unwrap_or(McpLevel::ReadOnly);
        Ok(ListToolsResult::with_all_items(
            TOOLS.iter().filter(|t| t.min <= level).map(tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let def = TOOLS
            .iter()
            .find(|t| t.name == request.name)
            .ok_or_else(|| {
                ErrorData::invalid_params(format!("unknown tool {}", request.name), None)
            })?;
        let client = client_name(&context);
        let level = match self.level(&client) {
            Ok(level) => level,
            Err(message) => return Ok(failure(message).into()),
        };
        if def.min > level {
            return Err(ErrorData::invalid_params(
                format!("tool {} needs level {}", def.name, level_name(def.min)),
                None,
            ));
        }
        Ok(self.run(def, request.arguments, &client).into())
    }
}
