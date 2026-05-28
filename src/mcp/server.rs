//! MCP server dispatch -- handles initialize, tools/list, tools/call.

use serde_json::{Value, json};

use super::protocol::JsonRpcResponse;
use super::tools::{self, ToolContext};

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "oobo";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Server {
    ctx: ToolContext,
    has_api_key: bool,
    has_repo: bool,
    instructions: String,
}

impl Server {
    pub fn new(
        api_key: Option<String>,
        api_url: String,
        project_root: Option<String>,
        branch: Option<String>,
    ) -> Self {
        let has_api_key = api_key.as_ref().is_some_and(|k| !k.is_empty());
        let has_repo = project_root.is_some();

        let instructions = build_instructions(has_api_key, has_repo, &project_root, &branch);

        let ctx = ToolContext::new(project_root, api_key, api_url);

        Self {
            ctx,
            has_api_key,
            has_repo,
            instructions,
        }
    }

    pub fn handle(
        &self,
        method: &str,
        id: Option<Value>,
        params: Option<&Value>,
    ) -> Option<JsonRpcResponse> {
        let id = id?; // notification, no response

        match method {
            "initialize" => Some(self.handle_initialize(id)),
            "tools/list" => Some(self.handle_tools_list(id)),
            "tools/call" => Some(self.handle_tools_call(id, params)),
            "ping" => Some(JsonRpcResponse::success(id, json!({}))),
            _ => Some(JsonRpcResponse::error(id, -32601, "Method not found")),
        }
    }

    fn handle_initialize(&self, id: Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION
                },
                "instructions": self.instructions
            }),
        )
    }

    fn handle_tools_list(&self, id: Value) -> JsonRpcResponse {
        let schemas = tools::tool_schemas(self.has_api_key, self.has_repo);
        JsonRpcResponse::success(id, json!({ "tools": schemas }))
    }

    fn handle_tools_call(&self, id: Value, params: Option<&Value>) -> JsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => {
                return JsonRpcResponse::success(
                    id,
                    json!({
                        "content": [{"type": "text", "text": "Missing params"}],
                        "isError": true
                    }),
                );
            }
        };

        let tool_name = params
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let result = tools::dispatch(&self.ctx, tool_name, &arguments);
        JsonRpcResponse::success(id, result)
    }
}

fn build_instructions(
    has_api_key: bool,
    has_repo: bool,
    project_root: &Option<String>,
    branch: &Option<String>,
) -> String {
    let mut parts = Vec::new();

    if let Some(root) = project_root {
        let project_name = std::path::Path::new(root)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let branch_str = branch.as_deref().unwrap_or("unknown");
        parts.push(format!("Project: {project_name} (branch: {branch_str})."));
    }

    if has_repo && has_api_key {
        parts.push(
            "You have access to local code search and engineering memory. \
             Use `search` to find code by intent or symbol name - prefer it over Grep/Glob/Read \
             for any question about how code works. \
             Use `get_context` at the start of any non-trivial task to get relevant history, \
             past decisions, and known pitfalls for the files you're working on. \
             Use `recall` to find past sessions, decisions, and learnings. \
             Use `ask` for high-level questions about the team's work."
                .to_string(),
        );
    } else if has_repo {
        parts.push(
            "You have access to local code search. \
             Use `search` to find code by intent or symbol name - prefer it over Grep/Glob/Read \
             for any question about how code works. \
             Cloud memory (recall, get_context, ask) is not configured - set OOBO_API_KEY to enable."
                .to_string(),
        );
    } else if has_api_key {
        parts.push(
            "You have access to engineering memory for the team. \
             Use `get_context` at the start of any task to get relevant history and context. \
             Use `recall` to find past sessions, decisions, and learnings. \
             Use `ask` for high-level questions about the team's engineering work. \
             Specify a `project` parameter to scope searches to a specific project."
                .to_string(),
        );
    }

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_protocol_version() {
        let server = Server::new(None, "https://api.oobo.ai".into(), None, None);
        let resp = server.handle("initialize", Some(json!(1)), None).unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], "oobo");
    }

    #[test]
    fn tools_list_varies_by_capabilities() {
        let server_no_key = Server::new(None, "https://api.oobo.ai".into(), Some("/tmp".into()), None);
        let resp = server_no_key.handle("tools/list", Some(json!(2)), None).unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"search"));
        assert!(names.contains(&"find_related"));
        assert!(!names.contains(&"recall"));
        assert!(!names.contains(&"ask"));

        let server_full = Server::new(
            Some("sk_test".into()),
            "https://api.oobo.ai".into(),
            Some("/tmp".into()),
            Some("main".into()),
        );
        let resp = server_full.handle("tools/list", Some(json!(3)), None).unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"search"));
        assert!(names.contains(&"find_related"));
        assert!(names.contains(&"recall"));
        assert!(names.contains(&"get_context"));
        assert!(names.contains(&"ask"));
    }

    #[test]
    fn notifications_return_none() {
        let server = Server::new(None, "https://api.oobo.ai".into(), None, None);
        let resp = server.handle("notifications/initialized", None, None);
        assert!(resp.is_none());
    }

    #[test]
    fn unknown_method_returns_error() {
        let server = Server::new(None, "https://api.oobo.ai".into(), None, None);
        let resp = server.handle("unknown/method", Some(json!(5)), None).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn ping_returns_empty() {
        let server = Server::new(None, "https://api.oobo.ai".into(), None, None);
        let resp = server.handle("ping", Some(json!(6)), None).unwrap();
        assert_eq!(resp.result.unwrap(), json!({}));
    }

    #[test]
    fn cloud_tools_without_key_returns_error() {
        let server = Server::new(None, "https://api.oobo.ai".into(), Some("/tmp".into()), None);
        let resp = server
            .handle(
                "tools/call",
                Some(json!(7)),
                Some(&json!({"name": "recall", "arguments": {"query": "test"}})),
            )
            .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("OOBO_API_KEY"));
    }
}
