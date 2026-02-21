use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use zagent_core::config::McpServerConfig;

pub struct McpManager {
    statuses: RwLock<BTreeMap<String, McpServerStatus>>,
    tool_to_server: HashMap<String, String>,
    peers: HashMap<String, rmcp::Peer<rmcp::RoleClient>>,
    // Keep running services alive for the process lifetime.
    _services: Vec<RunningService<rmcp::RoleClient, ()>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub enabled: bool,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl McpManager {
    pub async fn start_servers(
        servers: &std::collections::BTreeMap<String, McpServerConfig>,
        working_dir: &str,
    ) -> Self {
        let mut statuses = BTreeMap::<String, McpServerStatus>::new();
        let mut tool_to_server = HashMap::<String, String>::new();
        let mut peers = HashMap::<String, rmcp::Peer<rmcp::RoleClient>>::new();
        let mut services = Vec::<RunningService<rmcp::RoleClient, ()>>::new();

        for (name, cfg) in servers {
            if !cfg.is_enabled() {
                statuses.insert(
                    name.clone(),
                    McpServerStatus {
                        name: name.clone(),
                        enabled: false,
                        state: "disabled".to_string(),
                        tool_count: None,
                        tool_names: None,
                        error: None,
                    },
                );
                continue;
            }
            if cfg.command.trim().is_empty() {
                warn!(server = %name, "Skipping MCP server with empty command");
                statuses.insert(
                    name.clone(),
                    McpServerStatus {
                        name: name.clone(),
                        enabled: true,
                        state: "invalid_config".to_string(),
                        tool_count: None,
                        tool_names: None,
                        error: Some("empty command".to_string()),
                    },
                );
                continue;
            }

            match connect_server(name, cfg, working_dir).await {
                Ok((service, tool_names)) => {
                    for tool in &tool_names {
                        tool_to_server.insert(tool.clone(), name.clone());
                    }
                    peers.insert(name.clone(), service.peer().clone());
                    services.push(service);
                    statuses.insert(
                        name.clone(),
                        McpServerStatus {
                            name: name.clone(),
                            enabled: true,
                            state: "connected".to_string(),
                            tool_count: Some(tool_names.len()),
                            tool_names: Some(tool_names),
                            error: None,
                        },
                    );
                }
                Err(err) => {
                    error!(server = %name, error = %err, "MCP server failed");
                    statuses.insert(
                        name.clone(),
                        McpServerStatus {
                            name: name.clone(),
                            enabled: true,
                            state: "failed".to_string(),
                            tool_count: None,
                            tool_names: None,
                            error: Some(err.to_string()),
                        },
                    );
                }
            }
        }

        Self {
            statuses: RwLock::new(statuses),
            tool_to_server,
            peers,
            _services: services,
        }
    }

    pub async fn snapshot(&self) -> Vec<McpServerStatus> {
        self.statuses.read().await.values().cloned().collect()
    }

    pub async fn connected_tool_names(&self) -> Vec<String> {
        let statuses = self.statuses.read().await;
        let mut names = Vec::new();
        for status in statuses.values() {
            if status.state != "connected" {
                continue;
            }
            if let Some(tool_names) = &status.tool_names {
                names.extend(tool_names.clone());
            }
        }
        names.sort();
        names.dedup();
        names
    }

    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: Value,
    ) -> Result<String, zagent_core::Error> {
        let server_name = self.tool_to_server.get(tool_name).ok_or_else(|| {
            zagent_core::Error::tool(tool_name, format!("Unknown MCP tool: {tool_name}"))
        })?;
        let peer = self.peers.get(server_name).ok_or_else(|| {
            zagent_core::Error::provider("mcp", format!("MCP server '{server_name}' peer missing"))
        })?;

        let arguments = match args {
            Value::Null => None,
            Value::Object(map) => Some(map),
            other => {
                return Err(zagent_core::Error::tool(
                    tool_name,
                    format!("MCP tool arguments must be an object, got: {other}"),
                ));
            }
        };
        let result = peer
            .call_tool(CallToolRequestParams {
                meta: None,
                name: tool_name.to_string().into(),
                arguments,
                task: None,
            })
            .await
            .map_err(|e| zagent_core::Error::tool(tool_name, format!("MCP call failed: {e}")))?;

        serde_json::to_string_pretty(&result).map_err(zagent_core::Error::Json)
    }
}

async fn connect_server(
    name: &str,
    cfg: &McpServerConfig,
    working_dir: &str,
) -> Result<(RunningService<rmcp::RoleClient, ()>, Vec<String>), zagent_core::Error> {
    let mut command = Command::new(&cfg.command);
    command.args(&cfg.args);
    for (k, v) in &cfg.env {
        command.env(k, v);
    }

    let cwd = cfg
        .cwd
        .as_deref()
        .map(|raw| resolve_cwd(raw, working_dir))
        .transpose()?;
    command.current_dir(cwd.unwrap_or_else(|| Path::new(working_dir).to_path_buf()));
    let transport = TokioChildProcess::new(command.configure(|cmd| {
        cmd.stderr(std::process::Stdio::inherit());
    }))
    .map_err(|e| zagent_core::Error::config(format!("Failed to spawn MCP server '{name}': {e}")))?;

    let service = ().serve(transport).await.map_err(|e| {
        zagent_core::Error::provider("mcp", format!("Connect failed for '{name}': {e}"))
    })?;
    let tools = service.list_all_tools().await.map_err(|e| {
        zagent_core::Error::provider("mcp", format!("Tool listing failed for '{name}': {e}"))
    })?;
    let mut tool_names = tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>();
    tool_names.sort();

    info!(
        server = %name,
        tool_count = tool_names.len(),
        "Connected to MCP server"
    );
    Ok((service, tool_names))
}

fn resolve_cwd(raw: &str, base_dir: &str) -> Result<std::path::PathBuf, zagent_core::Error> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(Path::new(base_dir).join(path))
}
