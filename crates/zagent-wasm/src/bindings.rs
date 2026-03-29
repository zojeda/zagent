use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use js_sys::{Array, Function, Promise};
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use zagent_core::fs::{AgentFileSystem, FileSystemEntry};
use zagent_core::provider::Provider;
use zagent_core::provider::local::LocalProvider;
use zagent_core::provider::openai::OpenAiProvider;
use zagent_core::provider::openrouter::OpenRouterProvider;
use zagent_loop::{ContextManagementPolicy, LoopAgent, LoopAgentOptions};

use crate::http::BrowserHttpClient;

#[derive(Debug, Deserialize)]
struct WasmAgentOptions {
    provider: String,
    model: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    session_name: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    max_turns: Option<u32>,
    #[serde(default, alias = "contextManagementPolicy")]
    context_management_policy: Option<ContextManagementPolicy>,
}

#[wasm_bindgen]
pub struct WasmEmbeddedAgent {
    inner: LoopAgent,
}

#[wasm_bindgen]
impl WasmEmbeddedAgent {
    #[wasm_bindgen(js_name = create)]
    pub async fn create(
        options: JsValue,
        file_system: JsValue,
    ) -> Result<WasmEmbeddedAgent, JsValue> {
        console_error_panic_hook::set_once();

        let options: WasmAgentOptions = serde_wasm_bindgen::from_value(options)?;
        let provider_name = options.provider.clone();
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert(provider_name.clone(), build_provider(&options)?);

        let mut loop_options = LoopAgentOptions::new(
            provider_name.clone(),
            options.model,
            options
                .session_name
                .unwrap_or_else(|| "loop".to_string()),
            options.working_dir.unwrap_or_else(|| ".".to_string()),
        );
        loop_options.system_prompt = options.system_prompt;
        loop_options.max_turns = options.max_turns.unwrap_or(50);
        loop_options.context_management_policy =
            options.context_management_policy.unwrap_or_default();

        let inner = LoopAgent::new(
            Arc::new(BrowserHttpClient),
            providers,
            Arc::new(JsAgentFileSystem::new(file_system)?),
            loop_options,
        )
        .map_err(js_error)?;

        Ok(Self { inner })
    }

    #[wasm_bindgen(js_name = sendInput)]
    pub async fn send_input(&self, input: String) -> Result<JsValue, JsValue> {
        let response = self.inner.send_input(&input).await.map_err(js_error)?;
        serde_wasm_bindgen::to_value(&response).map_err(Into::into)
    }

    #[wasm_bindgen(js_name = toolNames)]
    pub fn tool_names(&self) -> Array {
        self.inner
            .tool_names()
            .into_iter()
            .map(JsValue::from_str)
            .collect()
    }
}

struct JsAgentFileSystem {
    read_file: Function,
    write_file: Function,
    list_dir: Function,
}

impl JsAgentFileSystem {
    fn new(file_system: JsValue) -> Result<Self, JsValue> {
        Ok(Self {
            read_file: get_function(&file_system, "readFile")?,
            write_file: get_function(&file_system, "writeFile")?,
            list_dir: get_function(&file_system, "listDir")?,
        })
    }
}

unsafe impl Send for JsAgentFileSystem {}
unsafe impl Sync for JsAgentFileSystem {}

#[async_trait(?Send)]
impl AgentFileSystem for JsAgentFileSystem {
    async fn read_to_string(&self, path: &str) -> zagent_core::Result<String> {
        let value = await_js(
            self.read_file
                .call1(&JsValue::NULL, &JsValue::from_str(path))
                .map_err(js_error)?,
        )
        .await?;
        value
            .as_string()
            .ok_or_else(|| zagent_core::Error::custom("readFile must resolve to a string"))
    }

    async fn write_string(&self, path: &str, content: &str) -> zagent_core::Result<()> {
        await_js(
            self.write_file
                .call2(
                    &JsValue::NULL,
                    &JsValue::from_str(path),
                    &JsValue::from_str(content),
                )
                .map_err(js_error)?,
        )
        .await?;
        Ok(())
    }

    async fn list_dir(
        &self,
        path: &str,
        recursive: bool,
        max_depth: usize,
    ) -> zagent_core::Result<Vec<FileSystemEntry>> {
        let value = await_js(
            self.list_dir
                .call3(
                    &JsValue::NULL,
                    &JsValue::from_str(path),
                    &JsValue::from_bool(recursive),
                    &JsValue::from_f64(max_depth as f64),
                )
                .map_err(js_error)?,
        )
        .await?;
        serde_wasm_bindgen::from_value(value)
            .map_err(|e| zagent_core::Error::custom(format!("Invalid listDir response: {e}")))
    }
}

fn get_function(target: &JsValue, name: &str) -> Result<Function, JsValue> {
    js_sys::Reflect::get(target, &JsValue::from_str(name))?
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str(&format!("file system adapter is missing {name}()")))
}

async fn await_js(value: JsValue) -> zagent_core::Result<JsValue> {
    if let Ok(promise) = value.dyn_into::<Promise>() {
        JsFuture::from(promise).await.map_err(|e| {
            zagent_core::Error::custom(format!("JavaScript host call failed: {:?}", e))
        })
    } else {
        Ok(value)
    }
}

fn build_provider(options: &WasmAgentOptions) -> Result<Arc<dyn Provider>, JsValue> {
    match options.provider.as_str() {
        "openai" => {
            let api_key = options
                .api_key
                .clone()
                .ok_or_else(|| JsValue::from_str("openai provider requires api_key"))?;
            let provider = options
                .base_url
                .clone()
                .map(|url| OpenAiProvider::new_api_key(api_key).with_base_url(url))
                .unwrap_or_else(|| OpenAiProvider::new_api_key(api_key));
            Ok(Arc::new(provider))
        }
        "openrouter" => {
            let api_key = options
                .api_key
                .clone()
                .ok_or_else(|| JsValue::from_str("openrouter provider requires api_key"))?;
            let provider = options
                .base_url
                .clone()
                .map(|url| OpenRouterProvider::new(api_key).with_base_url(url))
                .unwrap_or_else(|| OpenRouterProvider::new(api_key));
            Ok(Arc::new(provider))
        }
        "local" => {
            let base_url = options
                .base_url
                .clone()
                .ok_or_else(|| JsValue::from_str("local provider requires base_url"))?;
            let provider = options
                .api_key
                .clone()
                .map(|key| LocalProvider::new(base_url.clone()).with_api_key(key))
                .unwrap_or_else(|| LocalProvider::new(base_url));
            Ok(Arc::new(provider))
        }
        other => Err(JsValue::from_str(&format!(
            "Unsupported provider '{other}'. Use openai, openrouter, or local."
        ))),
    }
}

fn js_error(err: impl ToString) -> JsValue {
    JsValue::from_str(&err.to_string())
}
