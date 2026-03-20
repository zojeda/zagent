use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

const CONFIG_FILE_NAME: &str = "zagent-config.yaml";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ZagentConfig {
    #[serde(default)]
    pub default_provider: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub auth_method: Option<ProviderAuthMethod>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub access_token_env: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub account_id_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub app_name: Option<String>,
    #[serde(default)]
    pub app_url: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
}

impl ProviderConfig {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn merge_from(&mut self, other: &ProviderConfig) {
        if other.enabled.is_some() {
            self.enabled = other.enabled;
        }
        if other.auth_method.is_some() {
            self.auth_method = other.auth_method;
        }
        if other.api_key.is_some() {
            self.api_key = other.api_key.clone();
        }
        if other.api_key_env.is_some() {
            self.api_key_env = other.api_key_env.clone();
        }
        if other.access_token.is_some() {
            self.access_token = other.access_token.clone();
        }
        if other.access_token_env.is_some() {
            self.access_token_env = other.access_token_env.clone();
        }
        if other.account_id.is_some() {
            self.account_id = other.account_id.clone();
        }
        if other.account_id_env.is_some() {
            self.account_id_env = other.account_id_env.clone();
        }
        if other.base_url.is_some() {
            self.base_url = other.base_url.clone();
        }
        if other.app_name.is_some() {
            self.app_name = other.app_name.clone();
        }
        if other.app_url.is_some() {
            self.app_url = other.app_url.clone();
        }
        if other.default_model.is_some() {
            self.default_model = other.default_model.clone();
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthMethod {
    ApiKey,
    ChatgptSubscription,
}

impl ZagentConfig {
    fn merge_from(&mut self, other: ZagentConfig) {
        if other.default_provider.is_some() {
            self.default_provider = other.default_provider;
        }
        if other.default_model.is_some() {
            self.default_model = other.default_model;
        }
        for (name, incoming) in other.providers {
            self.providers
                .entry(name)
                .and_modify(|existing| existing.merge_from(&incoming))
                .or_insert(incoming);
        }
        for (name, incoming) in other.mcp_servers {
            self.mcp_servers
                .entry(name)
                .and_modify(|existing| existing.merge_from(&incoming))
                .or_insert(incoming);
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpServerConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

impl McpServerConfig {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn merge_from(&mut self, other: &McpServerConfig) {
        if other.enabled.is_some() {
            self.enabled = other.enabled;
        }
        if !other.command.trim().is_empty() {
            self.command = other.command.clone();
        }
        if !other.args.is_empty() {
            self.args = other.args.clone();
        }
        if !other.env.is_empty() {
            self.env = other.env.clone();
        }
        if other.cwd.is_some() {
            self.cwd = other.cwd.clone();
        }
    }
}

pub fn load_config(working_dir: &str) -> Result<ZagentConfig> {
    let mut merged = ZagentConfig::default();
    for path in config_paths(working_dir) {
        if !path.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            Error::config(format!(
                "failed to read config file {}: {e}",
                path.to_string_lossy()
            ))
        })?;
        let parsed = serde_yaml::from_str::<ZagentConfig>(&raw).map_err(|e| {
            Error::config(format!(
                "invalid yaml in config file {}: {e}",
                path.to_string_lossy()
            ))
        })?;
        merged.merge_from(parsed);
    }
    Ok(merged)
}

fn config_paths(working_dir: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = home_dir() {
        out.push(home.join(".config").join("zagent").join(CONFIG_FILE_NAME));
    }
    out.push(Path::new(working_dir).join(CONFIG_FILE_NAME));
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}
