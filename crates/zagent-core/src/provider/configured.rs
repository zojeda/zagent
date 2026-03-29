use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::Provider;
use super::local::LocalProvider;
use super::openai::OpenAiProvider;
use super::openrouter::OpenRouterProvider;
use crate::config::{ProviderAuthMethod, ProviderConfig, ZagentConfig};
use crate::{Error, Result};
use tracing::warn;

pub type SharedProviderMap = HashMap<String, Arc<dyn Provider>>;

pub fn build_configured_providers(
    app_config: &ZagentConfig,
    working_dir: &str,
) -> Result<SharedProviderMap> {
    let mut providers: SharedProviderMap = HashMap::new();
    let auth_file = resolve_auth_file(working_dir)?;

    for (provider_name, provider_config) in &app_config.providers {
        if !provider_config.is_enabled() {
            continue;
        }
        let provider_result: Result<Option<Arc<dyn Provider>>> = match provider_name.as_str() {
            "openrouter" => (|| {
                let api_key = resolve_provider_api_key(
                    provider_name,
                    provider_config,
                    auth_file.provider(provider_name),
                )?;
                let mut provider = OpenRouterProvider::new(api_key);
                if let Some(base_url) = resolve_provider_base_url(provider_name, provider_config) {
                    provider = provider.with_base_url(base_url);
                }
                if let Some(app_name) = resolve_provider_app_name(provider_name, provider_config) {
                    provider = provider.with_app_name(app_name);
                }
                if let Some(app_url) = resolve_provider_app_url(provider_name, provider_config) {
                    provider = provider.with_app_url(app_url);
                }
                let provider: Arc<dyn Provider> = Arc::new(provider);
                Ok(Some(provider))
            })(),
            "local" => (|| {
                let mut provider = LocalProvider::new(resolve_required_provider_base_url(
                    provider_name,
                    provider_config,
                )?);
                if let Some(api_key) = resolve_optional_provider_api_key(
                    provider_name,
                    provider_config,
                    auth_file.provider(provider_name),
                ) {
                    provider = provider.with_api_key(api_key);
                }
                let provider: Arc<dyn Provider> = Arc::new(provider);
                Ok(Some(provider))
            })(),
            "openai" => (|| {
                let auth_method = resolve_provider_auth_method(
                    provider_name,
                    provider_config,
                    auth_file.provider(provider_name),
                )?;
                let mut provider = match auth_method {
                    ProviderAuthMethod::ApiKey => {
                        OpenAiProvider::new_api_key(resolve_provider_api_key(
                            provider_name,
                            provider_config,
                            auth_file.provider(provider_name),
                        )?)
                    }
                    ProviderAuthMethod::ChatgptSubscription => {
                        OpenAiProvider::new_chatgpt_subscription(
                            resolve_provider_access_token(
                                provider_name,
                                provider_config,
                                auth_file.provider(provider_name),
                            )?,
                            resolve_provider_account_id(
                                provider_name,
                                provider_config,
                                auth_file.provider(provider_name),
                            )?,
                        )
                    }
                };
                if let Some(base_url) = resolve_provider_base_url(provider_name, provider_config) {
                    provider = provider.with_base_url(base_url);
                }
                let provider: Arc<dyn Provider> = Arc::new(provider);
                Ok(Some(provider))
            })(),
            _ => Ok(None),
        };

        match provider_result {
            Ok(Some(provider)) => {
                providers.insert(provider_name.clone(), provider);
            }
            Ok(None) => {}
            Err(err) => {
                warn!(
                    provider = %provider_name,
                    error = %err,
                    "Skipping misconfigured provider"
                );
            }
        }
    }

    if providers.is_empty() {
        if let Ok(api_key) = std::env::var("OPENROUTER_API_KEY")
            && !api_key.trim().is_empty()
        {
            providers.insert(
                "openrouter".to_string(),
                Arc::new(OpenRouterProvider::new(api_key)),
            );
        } else if let Some(auth) = auth_file.provider("openrouter")
            && let Some(api_key) = auth.api_key.as_ref()
        {
            providers.insert(
                "openrouter".to_string(),
                Arc::new(OpenRouterProvider::new(api_key.clone())),
            );
        }
        if let Ok(api_key) = std::env::var("OPENAI_API_KEY")
            && !api_key.trim().is_empty()
        {
            providers.insert(
                "openai".to_string(),
                Arc::new(OpenAiProvider::new_api_key(api_key)),
            );
        } else if let Some(auth) = auth_file.provider("openai") {
            if let Some(provider) = build_openai_provider_from_auth_file(auth)? {
                providers.insert("openai".to_string(), Arc::new(provider));
            }
        }
    }

    Ok(providers)
}

pub fn select_initial_provider(
    default_provider: Option<&str>,
    model: Option<&str>,
    providers: &SharedProviderMap,
) -> Result<String> {
    if let Ok(from_env_model) = std::env::var("ZAGENT_DEFAULT_MODEL")
        && let Some(provider_name) = infer_provider_from_model(from_env_model.trim(), providers)
    {
        return Ok(provider_name);
    }
    if let Ok(from_env) = std::env::var("ZAGENT_DEFAULT_PROVIDER")
        && providers.contains_key(from_env.trim())
    {
        return Ok(from_env.trim().to_string());
    }
    if let Some(from_config) = default_provider
        && providers.contains_key(from_config.trim())
    {
        return Ok(from_config.trim().to_string());
    }
    if let Some((provider_name, _)) = split_provider_model(model.unwrap_or_default())
        && providers.contains_key(provider_name)
    {
        return Ok(provider_name.to_string());
    }
    if providers.contains_key("openrouter") {
        return Ok("openrouter".to_string());
    }
    let mut names: Vec<String> = providers.keys().cloned().collect();
    names.sort();
    names
        .into_iter()
        .next()
        .ok_or_else(|| Error::config("No providers configured"))
}

pub fn ensure_requested_provider_available(
    default_provider: Option<&str>,
    model: Option<&str>,
    _app_config: &ZagentConfig,
    providers: &SharedProviderMap,
) -> Result<()> {
    if let Some(requested) = default_provider.map(str::trim)
        && !requested.is_empty()
        && !providers.contains_key(requested)
    {
        return Err(Error::config(format!(
            "Configured default provider '{requested}' is not available. Check its configuration and authentication."
        )));
    }

    if let Ok(from_env) = std::env::var("ZAGENT_DEFAULT_PROVIDER") {
        let requested = from_env.trim();
        if !requested.is_empty() && !providers.contains_key(requested) {
            return Err(Error::config(format!(
                "Requested default provider '{requested}' is not available. Check its configuration and authentication."
            )));
        }
    }

    if let Some((provider_name, _)) = split_provider_model(model.unwrap_or_default())
        && !providers.contains_key(provider_name)
    {
        return Err(Error::config(format!(
            "Requested provider '{provider_name}' for model '{}' is not available. Check its configuration and authentication.",
            model.unwrap_or_default().trim()
        )));
    }

    Ok(())
}

pub fn resolve_default_model(provider_name: &str, app_config: &ZagentConfig) -> Result<String> {
    if let Ok(model) = std::env::var("ZAGENT_DEFAULT_MODEL")
        && !model.trim().is_empty()
    {
        return Ok(model);
    }
    if let Ok(model) = std::env::var(provider_env_var(provider_name, "DEFAULT_MODEL"))
        && !model.trim().is_empty()
    {
        return Ok(model);
    }
    if let Some(model) = app_config
        .providers
        .get(provider_name)
        .and_then(|p| p.default_model.clone())
        .filter(|m| !m.trim().is_empty())
    {
        return Ok(model);
    }
    if let Some(model) = app_config
        .default_model
        .clone()
        .filter(|m| !m.trim().is_empty())
    {
        return Ok(model);
    }
    match provider_name {
        "openrouter" => Ok("anthropic/claude-sonnet-4".to_string()),
        "openai" => Ok("gpt-5.2".to_string()),
        _ => Err(missing_default_model_error(provider_name)),
    }
}

fn missing_default_model_error(provider_name: &str) -> Error {
    Error::config(format!(
        "No default model configured for provider '{provider_name}'. Set {} or configure providers.{provider_name}.default_model or default_model in zagent-config.yaml",
        provider_env_var(provider_name, "DEFAULT_MODEL")
    ))
}

pub fn resolve_workspace_default_model(
    app_config: &ZagentConfig,
    providers: &SharedProviderMap,
) -> Result<String> {
    if let Ok(model) = std::env::var("ZAGENT_DEFAULT_MODEL")
        && !model.trim().is_empty()
    {
        if let Some((provider_name, _)) = split_provider_model(&model) {
            if providers.contains_key(provider_name) {
                return Ok(model);
            }
        }
    }

    let provider_name =
        select_initial_provider(app_config.default_provider.as_deref(), None, providers)?;
    let model = resolve_default_model(&provider_name, app_config)?;

    if split_provider_model(&model).is_some() {
        return Ok(model);
    }

    Ok(format!("{provider_name}:{model}"))
}

pub fn split_provider_model(model: &str) -> Option<(&str, &str)> {
    let trimmed = model.trim();
    let (provider, model) = trimmed.split_once(':')?;
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some((provider, model))
}

fn infer_provider_from_model(model: &str, providers: &SharedProviderMap) -> Option<String> {
    if let Some((provider_name, _)) = split_provider_model(model)
        && providers.contains_key(provider_name)
    {
        return Some(provider_name.to_string());
    }

    let trimmed = model.trim();
    if trimmed.is_empty() {
        return None;
    }

    // OpenRouter model identifiers are typically expressed as `vendor/model`.
    if trimmed.contains('/') && providers.contains_key("openrouter") {
        return Some("openrouter".to_string());
    }

    None
}

pub fn provider_env_var(provider_name: &str, suffix: &str) -> String {
    let normalized: String = provider_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("ZAGENT_PROVIDER_{normalized}_{suffix}")
}

fn resolve_provider_auth_method(
    provider_name: &str,
    provider_config: &ProviderConfig,
    auth_file: Option<&ProviderAuthFile>,
) -> Result<ProviderAuthMethod> {
    if let Ok(value) = std::env::var(provider_env_var(provider_name, "AUTH_METHOD"))
        && !value.trim().is_empty()
    {
        return parse_auth_method(provider_name, &value);
    }
    if let Some(auth_method) = provider_config.auth_method {
        return Ok(auth_method);
    }
    if provider_name == "openai"
        && let Some(auth_file) = auth_file
        && let Some(auth_method) = auth_file.auth_method
    {
        return Ok(auth_method);
    }
    Ok(ProviderAuthMethod::ApiKey)
}

fn parse_auth_method(provider_name: &str, raw: &str) -> Result<ProviderAuthMethod> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "api_key" => Ok(ProviderAuthMethod::ApiKey),
        "chatgpt_subscription" => Ok(ProviderAuthMethod::ChatgptSubscription),
        other => Err(Error::config(format!(
            "Provider '{provider_name}' has invalid auth_method '{other}'. Expected 'api_key' or 'chatgpt_subscription'"
        ))),
    }
}

fn resolve_provider_api_key(
    provider_name: &str,
    provider_config: &ProviderConfig,
    auth_file: Option<&ProviderAuthFile>,
) -> Result<String> {
    let env_key = provider_env_var(provider_name, "API_KEY");
    if let Ok(value) = std::env::var(&env_key)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(custom_env_name) = provider_config.api_key_env.as_deref()
        && let Ok(value) = std::env::var(custom_env_name)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(raw) = provider_config.api_key.as_deref()
        && !raw.trim().is_empty()
    {
        return Ok(raw.to_string());
    }
    if let Some(api_key) = auth_file.and_then(|auth| auth.api_key.as_deref()) {
        return Ok(api_key.to_string());
    }
    match provider_name {
        "openrouter" => std::env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                Error::config(format!(
                    "Provider '{provider_name}' is missing an API key. Set {} or OPENROUTER_API_KEY, or configure api_key/api_key_env in zagent-config.yaml, or add api_key to auth.json",
                    provider_env_var(provider_name, "API_KEY")
                ))
            }),
        "openai" => std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                Error::config(format!(
                    "Provider '{provider_name}' is missing an API key. Set {} or OPENAI_API_KEY, or configure api_key/api_key_env in zagent-config.yaml, or add api_key to auth.json",
                    provider_env_var(provider_name, "API_KEY")
                ))
            }),
        _ => Err(Error::config(format!(
            "Provider '{provider_name}' is missing an API key. Set {} or configure api_key/api_key_env in zagent-config.yaml",
            provider_env_var(provider_name, "API_KEY")
        ))),
    }
}

fn resolve_optional_provider_api_key(
    provider_name: &str,
    provider_config: &ProviderConfig,
    auth_file: Option<&ProviderAuthFile>,
) -> Option<String> {
    let env_key = provider_env_var(provider_name, "API_KEY");
    std::env::var(&env_key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            provider_config
                .api_key_env
                .as_deref()
                .and_then(|name| std::env::var(name).ok())
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            provider_config
                .api_key
                .clone()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| auth_file.and_then(|auth| auth.api_key.clone()))
}

fn resolve_required_provider_base_url(
    provider_name: &str,
    provider_config: &ProviderConfig,
) -> Result<String> {
    resolve_provider_base_url(provider_name, provider_config).ok_or_else(|| {
        Error::config(format!(
            "Provider '{provider_name}' requires base_url. Set {} or configure providers.{provider_name}.base_url in zagent-config.yaml",
            provider_env_var(provider_name, "BASE_URL")
        ))
    })
}

fn resolve_provider_access_token(
    provider_name: &str,
    provider_config: &ProviderConfig,
    auth_file: Option<&ProviderAuthFile>,
) -> Result<String> {
    let env_key = provider_env_var(provider_name, "ACCESS_TOKEN");
    if let Ok(value) = std::env::var(&env_key)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(custom_env_name) = provider_config.access_token_env.as_deref()
        && let Ok(value) = std::env::var(custom_env_name)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(raw) = provider_config.access_token.as_deref()
        && !raw.trim().is_empty()
    {
        return Ok(raw.to_string());
    }
    if provider_name == "openai"
        && let Some(access_token) = auth_file.and_then(|auth| auth.access_token.as_deref())
    {
        return Ok(access_token.to_string());
    }

    Err(Error::config(format!(
        "Provider '{provider_name}' is missing a ChatGPT access token. Set {} or configure access_token/access_token_env in zagent-config.yaml, or add access_token to auth.json",
        env_key
    )))
}

fn resolve_provider_account_id(
    provider_name: &str,
    provider_config: &ProviderConfig,
    auth_file: Option<&ProviderAuthFile>,
) -> Result<String> {
    let env_key = provider_env_var(provider_name, "ACCOUNT_ID");
    if let Ok(value) = std::env::var(&env_key)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(custom_env_name) = provider_config.account_id_env.as_deref()
        && let Ok(value) = std::env::var(custom_env_name)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(raw) = provider_config.account_id.as_deref()
        && !raw.trim().is_empty()
    {
        return Ok(raw.to_string());
    }
    if provider_name == "openai"
        && let Some(account_id) = auth_file.and_then(|auth| auth.account_id.as_deref())
    {
        return Ok(account_id.to_string());
    }

    Err(Error::config(format!(
        "Provider '{provider_name}' is missing a ChatGPT account/workspace id. Set {} or configure account_id/account_id_env in zagent-config.yaml, or add account_id to auth.json",
        env_key
    )))
}

fn resolve_provider_base_url(
    provider_name: &str,
    provider_config: &ProviderConfig,
) -> Option<String> {
    std::env::var(provider_env_var(provider_name, "BASE_URL"))
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| provider_config.base_url.clone())
}

fn resolve_provider_app_name(
    provider_name: &str,
    provider_config: &ProviderConfig,
) -> Option<String> {
    std::env::var(provider_env_var(provider_name, "APP_NAME"))
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| provider_config.app_name.clone())
}

fn resolve_provider_app_url(
    provider_name: &str,
    provider_config: &ProviderConfig,
) -> Option<String> {
    std::env::var(provider_env_var(provider_name, "APP_URL"))
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| provider_config.app_url.clone())
}

#[derive(Debug, Clone)]
struct ProviderAuthFile {
    auth_method: Option<ProviderAuthMethod>,
    api_key: Option<String>,
    access_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct AuthFile {
    providers: HashMap<String, ProviderAuthFile>,
}

impl AuthFile {
    fn provider(&self, provider_name: &str) -> Option<&ProviderAuthFile> {
        self.providers.get(provider_name)
    }
}

fn build_openai_provider_from_auth_file(
    auth_file: &ProviderAuthFile,
) -> Result<Option<OpenAiProvider>> {
    match auth_file
        .auth_method
        .unwrap_or_else(|| infer_openai_auth_method(auth_file))
    {
        ProviderAuthMethod::ApiKey => Ok(auth_file
            .api_key
            .as_ref()
            .map(|api_key| OpenAiProvider::new_api_key(api_key.clone()))),
        ProviderAuthMethod::ChatgptSubscription => {
            if let (Some(access_token), Some(account_id)) =
                (&auth_file.access_token, &auth_file.account_id)
            {
                Ok(Some(OpenAiProvider::new_chatgpt_subscription(
                    access_token.clone(),
                    account_id.clone(),
                )))
            } else {
                Ok(None)
            }
        }
    }
}

fn infer_openai_auth_method(auth_file: &ProviderAuthFile) -> ProviderAuthMethod {
    if auth_file.access_token.is_some() || auth_file.account_id.is_some() {
        ProviderAuthMethod::ChatgptSubscription
    } else {
        ProviderAuthMethod::ApiKey
    }
}

fn resolve_auth_file(working_dir: &str) -> Result<AuthFile> {
    for path in auth_file_paths(working_dir, home_dir().as_deref()) {
        if !path.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            Error::config(format!(
                "failed to read auth file {}: {e}",
                path.to_string_lossy()
            ))
        })?;
        let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
            Error::config(format!(
                "invalid json in auth file {}: {e}",
                path.to_string_lossy()
            ))
        })?;
        if let Some(auth) = parse_auth_file_value(&value, &path)? {
            return Ok(auth);
        }
    }
    Ok(AuthFile::default())
}

fn auth_file_paths(working_dir: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![
        Path::new(working_dir).join("auth.json"),
        Path::new(working_dir).join(".zagent").join("auth.json"),
    ];
    if let Some(home) = home {
        paths.push(home.join(".zagent").join("auth.json"));
    }
    paths
}

fn parse_auth_file_value(value: &serde_json::Value, path: &Path) -> Result<Option<AuthFile>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };

    if let Some(providers_value) = object.get("providers")
        && let Some(providers_object) = providers_value.as_object()
    {
        let mut providers = HashMap::new();
        for (provider_name, provider_value) in providers_object {
            if let Some(auth) = parse_provider_auth_value(provider_name, provider_value, path)? {
                providers.insert(provider_name.clone(), auth);
            }
        }
        if providers.is_empty() {
            return Ok(None);
        }
        return Ok(Some(AuthFile { providers }));
    }

    let mut providers = HashMap::new();
    for provider_name in ["openai", "openrouter", "local"] {
        let node = object.get(provider_name).unwrap_or(value);
        if let Some(auth) = parse_provider_auth_value(provider_name, node, path)? {
            providers.insert(provider_name.to_string(), auth);
        }
    }
    if providers.is_empty() {
        return Ok(None);
    }
    Ok(Some(AuthFile { providers }))
}

fn parse_provider_auth_value(
    provider_name: &str,
    value: &serde_json::Value,
    path: &Path,
) -> Result<Option<ProviderAuthFile>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };

    let auth_method = string_field(object, &["auth_method"])
        .or_else(|| string_field(object, &["auth_mode"]).and_then(parse_auth_mode_alias))
        .map(|value| parse_auth_method(provider_name, value))
        .transpose()?;
    let api_key = string_field(object, &["api_key"])
        .or_else(|| nested_string_field(object, "tokens", &["api_key"]));
    let access_token = string_field(object, &["access_token", "token"])
        .or_else(|| nested_string_field(object, "tokens", &["access_token", "token"]));
    let account_id = string_field(
        object,
        &["account_id", "workspace_id", "chatgpt_account_id"],
    )
    .or_else(|| {
        nested_string_field(
            object,
            "tokens",
            &["account_id", "workspace_id", "chatgpt_account_id"],
        )
    });

    if auth_method.is_none() && api_key.is_none() && access_token.is_none() && account_id.is_none()
    {
        return Ok(None);
    }

    if access_token.is_some()
        && account_id.is_none()
        && auth_method == Some(ProviderAuthMethod::ChatgptSubscription)
    {
        return Err(Error::config(format!(
            "Provider '{provider_name}' auth file {} is missing account_id for chatgpt_subscription auth",
            path.to_string_lossy()
        )));
    }

    Ok(Some(ProviderAuthFile {
        auth_method,
        api_key: api_key.map(ToOwned::to_owned),
        access_token: access_token.map(ToOwned::to_owned),
        account_id: account_id.map(ToOwned::to_owned),
    }))
}

fn string_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn nested_string_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    parent_key: &str,
    keys: &[&str],
) -> Option<&'a str> {
    object
        .get(parent_key)
        .and_then(serde_json::Value::as_object)
        .and_then(|nested| string_field(nested, keys))
}

fn parse_auth_mode_alias(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "chatgpt" | "chatgpt_subscription" => Some("chatgpt_subscription"),
        "api_key" => Some("api_key"),
        _ => None,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::provider::types::{ChatRequest, Message};
    use std::collections::BTreeMap;
    use std::sync::{Mutex, MutexGuard};
    use uuid::Uuid;

    static DEFAULT_MODEL_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn default_model_env_guard() -> MutexGuard<'static, ()> {
        DEFAULT_MODEL_ENV_LOCK.lock().expect("env lock")
    }

    fn config_with_provider(name: &str, provider: ProviderConfig) -> ZagentConfig {
        let mut providers = BTreeMap::new();
        providers.insert(name.to_string(), provider);
        ZagentConfig {
            default_provider: None,
            default_model: None,
            context_management_policy: Default::default(),
            providers,
            mcp_servers: BTreeMap::new(),
        }
    }

    #[test]
    fn openai_api_key_provider_uses_configured_key() {
        let providers = build_configured_providers(
            &config_with_provider(
                "openai",
                ProviderConfig {
                    api_key: Some("sk-test".to_string()),
                    ..ProviderConfig::default()
                },
            ),
            ".",
        )
        .expect("providers");

        let provider = providers.get("openai").expect("openai provider");
        let request = provider
            .build_http_request(&ChatRequest::new("gpt-5.2", vec![Message::user("hi")]))
            .expect("request");
        assert_eq!(request.url, "https://api.openai.com/v1/responses");
        assert!(
            request
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer sk-test")
        );
    }

    #[test]
    fn openai_subscription_provider_requires_access_token_and_account_id() {
        let providers = build_configured_providers(
            &config_with_provider(
                "openai",
                ProviderConfig {
                    auth_method: Some(ProviderAuthMethod::ChatgptSubscription),
                    access_token: Some("token".to_string()),
                    ..ProviderConfig::default()
                },
            ),
            ".",
        )
        .expect("providers");

        assert!(!providers.contains_key("openai"));
    }

    #[test]
    fn openai_subscription_provider_switches_base_url_and_headers() {
        let providers = build_configured_providers(
            &config_with_provider(
                "openai",
                ProviderConfig {
                    auth_method: Some(ProviderAuthMethod::ChatgptSubscription),
                    access_token: Some("token".to_string()),
                    account_id: Some("acct_123".to_string()),
                    ..ProviderConfig::default()
                },
            ),
            ".",
        )
        .expect("providers");

        let provider = providers.get("openai").expect("openai provider");
        let request = provider
            .build_http_request(&ChatRequest::new("gpt-5.2", vec![Message::user("hi")]))
            .expect("request");
        assert_eq!(
            request.url,
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert!(
            request
                .headers
                .iter()
                .any(|(k, v)| k == "ChatGPT-Account-Id" && v == "acct_123")
        );
    }

    #[test]
    fn openai_default_model_is_gpt_5_2() {
        let _guard = default_model_env_guard();
        assert_eq!(
            resolve_default_model("openai", &ZagentConfig::default()).expect("model"),
            "gpt-5.2"
        );
    }

    #[test]
    fn local_provider_requires_base_url() {
        let providers = build_configured_providers(
            &config_with_provider("local", ProviderConfig::default()),
            ".",
        )
        .expect("providers");

        assert!(!providers.contains_key("local"));
    }

    #[test]
    fn local_provider_uses_base_url_without_auth_header_when_no_key_configured() {
        let providers = build_configured_providers(
            &config_with_provider(
                "local",
                ProviderConfig {
                    base_url: Some("http://127.0.0.1:1234/v1".to_string()),
                    default_model: Some("qwen2.5-coder-7b-instruct".to_string()),
                    ..ProviderConfig::default()
                },
            ),
            ".",
        )
        .expect("providers");

        let provider = providers.get("local").expect("local provider");
        let request = provider
            .build_http_request(&ChatRequest::new(
                "qwen2.5-coder-7b-instruct",
                vec![Message::user("hi")],
            ))
            .expect("request");

        assert_eq!(request.url, "http://127.0.0.1:1234/v1/chat/completions");
        assert!(
            !request
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("Authorization"))
        );
    }

    #[test]
    fn local_provider_default_model_must_be_configured() {
        let _guard = default_model_env_guard();
        let err = resolve_default_model("local", &ZagentConfig::default())
            .expect_err("local default model should require configuration");
        assert!(err.to_string().contains("providers.local.default_model"));
    }

    #[test]
    fn split_provider_model_uses_colon_prefix() {
        assert_eq!(
            split_provider_model("openai:gpt-5.2"),
            Some(("openai", "gpt-5.2"))
        );
        assert_eq!(split_provider_model("gpt-5.2"), None);
    }

    #[test]
    fn workspace_default_model_keeps_configured_provider_prefix() {
        let _guard = default_model_env_guard();
        let providers = build_configured_providers(
            &config_with_provider(
                "openai",
                ProviderConfig {
                    api_key: Some("sk-test".to_string()),
                    default_model: Some("gpt-5.2".to_string()),
                    ..ProviderConfig::default()
                },
            ),
            ".",
        )
        .expect("providers");
        let config = ZagentConfig {
            default_provider: Some("openai".to_string()),
            default_model: None,
            context_management_policy: Default::default(),
            providers: {
                let mut providers = BTreeMap::new();
                providers.insert(
                    "openai".to_string(),
                    ProviderConfig {
                        api_key: Some("sk-test".to_string()),
                        default_model: Some("gpt-5.2".to_string()),
                        ..ProviderConfig::default()
                    },
                );
                providers
            },
            mcp_servers: BTreeMap::new(),
        };

        assert_eq!(
            resolve_workspace_default_model(&config, &providers).expect("model"),
            "openai:gpt-5.2"
        );
    }

    #[test]
    fn openai_subscription_provider_reads_auth_json_from_working_dir() {
        let temp_dir = std::env::temp_dir().join(format!("zagent-auth-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        std::fs::write(
            temp_dir.join("auth.json"),
            r#"{"openai":{"auth_method":"chatgpt_subscription","access_token":"token","account_id":"acct_123"}}"#,
        )
        .expect("auth.json");

        let providers = build_configured_providers(
            &config_with_provider("openai", ProviderConfig::default()),
            temp_dir.to_string_lossy().as_ref(),
        )
        .expect("providers");

        let provider = providers.get("openai").expect("openai provider");
        let request = provider
            .build_http_request(&ChatRequest::new("gpt-5.2", vec![Message::user("hi")]))
            .expect("request");
        assert_eq!(
            request.url,
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert!(
            request
                .headers
                .iter()
                .any(|(k, v)| k == "ChatGPT-Account-Id" && v == "acct_123")
        );

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn openai_subscription_provider_reads_provider_keyed_auth_json() {
        let temp_dir = std::env::temp_dir().join(format!("zagent-auth-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        std::fs::write(
            temp_dir.join("auth.json"),
            r#"{"providers":{"openai":{"auth_method":"chatgpt_subscription","access_token":"token","account_id":"acct_123"}}}"#,
        )
        .expect("auth.json");

        let providers = build_configured_providers(
            &config_with_provider("openai", ProviderConfig::default()),
            temp_dir.to_string_lossy().as_ref(),
        )
        .expect("providers");

        let provider = providers.get("openai").expect("openai provider");
        let request = provider
            .build_http_request(&ChatRequest::new("gpt-5.2", vec![Message::user("hi")]))
            .expect("request");
        assert_eq!(
            request.url,
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert!(
            request
                .headers
                .iter()
                .any(|(k, v)| k == "ChatGPT-Account-Id" && v == "acct_123")
        );

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn openai_subscription_provider_reads_nested_tokens_auth_json() {
        let temp_dir = std::env::temp_dir().join(format!("zagent-auth-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        std::fs::write(
            temp_dir.join("auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"token","account_id":"acct_123"}}"#,
        )
        .expect("auth.json");

        let providers = build_configured_providers(
            &config_with_provider("openai", ProviderConfig::default()),
            temp_dir.to_string_lossy().as_ref(),
        )
        .expect("providers");

        let provider = providers.get("openai").expect("openai provider");
        let request = provider
            .build_http_request(&ChatRequest::new("gpt-5.2", vec![Message::user("hi")]))
            .expect("request");
        assert_eq!(
            request.url,
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert!(
            request
                .headers
                .iter()
                .any(|(k, v)| k == "ChatGPT-Account-Id" && v == "acct_123")
        );

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn local_provider_reads_provider_keyed_auth_json() {
        let temp_dir = std::env::temp_dir().join(format!("zagent-auth-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        std::fs::write(
            temp_dir.join("auth.json"),
            r#"{"providers":{"local":{"api_key":"local-test"}}}"#,
        )
        .expect("auth.json");

        let providers = build_configured_providers(
            &config_with_provider(
                "local",
                ProviderConfig {
                    base_url: Some("http://127.0.0.1:1234/v1".to_string()),
                    ..ProviderConfig::default()
                },
            ),
            temp_dir.to_string_lossy().as_ref(),
        )
        .expect("providers");

        let provider = providers.get("local").expect("local provider");
        let request = provider
            .build_http_request(&ChatRequest::new("model", vec![Message::user("hi")]))
            .expect("request");
        assert!(
            request
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer local-test")
        );
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn openrouter_api_key_provider_reads_provider_keyed_auth_json() {
        let temp_dir = std::env::temp_dir().join(format!("zagent-auth-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        std::fs::write(
            temp_dir.join("auth.json"),
            r#"{"providers":{"openrouter":{"api_key":"or-test"}}}"#,
        )
        .expect("auth.json");

        let providers = build_configured_providers(
            &config_with_provider("openrouter", ProviderConfig::default()),
            temp_dir.to_string_lossy().as_ref(),
        )
        .expect("providers");

        let provider = providers.get("openrouter").expect("openrouter provider");
        let request = provider
            .build_http_request(&ChatRequest::new(
                "anthropic/claude-sonnet-4",
                vec![Message::user("hi")],
            ))
            .expect("request");
        assert!(
            request
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer or-test")
        );

        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn auth_file_paths_include_workspace_and_home_locations() {
        let paths = auth_file_paths("/workspace/project", Some(Path::new("/home/tester")));

        assert_eq!(paths[0], PathBuf::from("/workspace/project/auth.json"));
        assert_eq!(
            paths[1],
            PathBuf::from("/workspace/project/.zagent/auth.json")
        );
        assert_eq!(paths[2], PathBuf::from("/home/tester/.zagent/auth.json"));
    }

    #[test]
    fn env_default_model_with_vendor_prefix_selects_openrouter() {
        let _guard = default_model_env_guard();
        unsafe {
            std::env::set_var("ZAGENT_DEFAULT_MODEL", "minimax/minimax-m2.5");
        }

        let providers = build_configured_providers(
            &ZagentConfig {
                default_provider: Some("openai".to_string()),
                default_model: None,
                context_management_policy: Default::default(),
                providers: {
                    let mut providers = BTreeMap::new();
                    providers.insert(
                        "openai".to_string(),
                        ProviderConfig {
                            api_key: Some("sk-test".to_string()),
                            ..ProviderConfig::default()
                        },
                    );
                    providers.insert(
                        "openrouter".to_string(),
                        ProviderConfig {
                            api_key: Some("or-test".to_string()),
                            ..ProviderConfig::default()
                        },
                    );
                    providers
                },
                mcp_servers: BTreeMap::new(),
            },
            ".",
        )
        .expect("providers");

        let selected =
            select_initial_provider(Some("openai"), None, &providers).expect("selected provider");
        assert_eq!(selected, "openrouter");

        unsafe {
            std::env::remove_var("ZAGENT_DEFAULT_MODEL");
        }
    }

    #[test]
    fn explicit_default_provider_must_be_available() {
        let config = ZagentConfig {
            default_provider: Some("openai".to_string()),
            default_model: None,
            context_management_policy: Default::default(),
            providers: {
                let mut providers = BTreeMap::new();
                providers.insert("openai".to_string(), ProviderConfig::default());
                providers
            },
            mcp_servers: BTreeMap::new(),
        };

        let err = ensure_requested_provider_available(
            config.default_provider.as_deref(),
            None,
            &config,
            &HashMap::new(),
        )
        .expect_err("missing default provider should error");

        assert!(
            err.to_string()
                .contains("Configured default provider 'openai' is not available")
        );
    }
}
