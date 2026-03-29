use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
#[cfg(target_arch = "wasm32")]
use gloo_net::http::Request;
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    Document, HtmlButtonElement, HtmlElement, HtmlInputElement, HtmlSelectElement,
    HtmlTextAreaElement, Window,
};
use zagent_core::agent::AgentProgressEvent;
use zagent_core::provider::local::LocalProvider;
use zagent_core::provider::openai::OpenAiProvider;
use zagent_core::provider::openrouter::OpenRouterProvider;
use zagent_core::provider::{HttpClient, HttpRequest, HttpResponse, Provider};
use zagent_wasm::{LoopAgent, LoopAgentOptions, MemoryFileSystem};

const DEFAULT_VFS: &str = r##"{
  "AGENTS.md": "# Demo Rules\n- Only use files from the injected workspace.\n- Summarize what you can directly read.\n",
  "project/readme.md": "zAgent can run in the browser as a wasm module.\nThe host controls the visible files through a custom VFS.\n",
  "project/notes.txt": "Future work: replace the scripted provider with a fetch-backed browser provider and JS-owned filesystem callbacks.\n"
}"##;

const DEFAULT_PROMPT: &str = "Read the project readme and explain how this demo proves the browser only sees the injected VFS.";
const DEFAULT_PROVIDER: &str = "openrouter";
const DEFAULT_MODEL: &str = "openai/gpt-4o-mini";
const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Default)]
struct BrowserHttpClient;

#[cfg(target_arch = "wasm32")]
#[async_trait(?Send)]
impl HttpClient for BrowserHttpClient {
    async fn send(&self, request: HttpRequest) -> zagent_core::Result<HttpResponse> {
        let mut builder = match request.method {
            zagent_core::provider::HttpMethod::Get => Request::get(&request.url),
            zagent_core::provider::HttpMethod::Post => Request::post(&request.url),
        };

        for (key, value) in &request.headers {
            builder = builder.header(key, value);
        }

        let response = if let Some(body) = request.body {
            builder
                .body(body)
                .map_err(|e| {
                    zagent_core::Error::provider("browser", format!("request body failed: {e}"))
                })?
                .send()
                .await
        } else {
            builder.send().await
        }
        .map_err(|e| {
            zagent_core::Error::provider(
                "browser",
                format!("request failed. Check CORS, base URL, and API key: {e}"),
            )
        })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            zagent_core::Error::provider("browser", format!("failed to read body: {e}"))
        })?;

        Ok(HttpResponse {
            status,
            body,
            headers: Vec::new(),
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl HttpClient for BrowserHttpClient {
    async fn send(&self, _request: HttpRequest) -> zagent_core::Result<HttpResponse> {
        Err(zagent_core::Error::provider(
            "browser",
            "BrowserHttpClient is only available on wasm32 targets",
        ))
    }
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let document = document()?;
    let window = window()?;

    let vfs = textarea(&document, "vfs")?;
    let prompt = textarea(&document, "prompt")?;
    let provider = select(&document, "provider")?;
    let model = input(&document, "model")?;
    let base_url = input(&document, "base-url")?;
    let api_key = input(&document, "api-key")?;
    let output = element(&document, "output")?;
    let debug = element(&document, "debug")?;
    let meta = element(&document, "meta")?;
    let status = element(&document, "status")?;
    let run = button(&document, "run")?;
    let reset = button(&document, "reset")?;

    vfs.set_value(DEFAULT_VFS);
    prompt.set_value(DEFAULT_PROMPT);
    provider.set_value(DEFAULT_PROVIDER);
    model.set_value(DEFAULT_MODEL);
    base_url.set_value(DEFAULT_BASE_URL);
    api_key.set_value("");
    output.set_text_content(Some("Press “Run Agent” to start the demo."));
    debug.set_text_content(Some("No tool calls yet."));
    meta.set_text_content(Some(
        "The browser will construct a fresh loop agent for each run.",
    ));
    status.set_text_content(Some("Ready."));
    set_status_class(&status, Some("status-ok"))?;

    let shared = Rc::new(AppElements {
        vfs,
        prompt,
        provider,
        model,
        base_url,
        api_key,
        output,
        debug,
        meta,
        status,
        run,
        reset,
        _window: window,
    });

    {
        let app = shared.clone();
        let on_click = Closure::<dyn FnMut(_)>::wrap(Box::new(move |_event: web_sys::Event| {
            let app = app.clone();
            spawn_local(async move {
                run_demo(app).await;
            });
        }));
        shared
            .run
            .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())?;
        on_click.forget();
    }

    {
        let app = shared.clone();
        let on_click = Closure::<dyn FnMut(_)>::wrap(Box::new(move |_event: web_sys::Event| {
            app.vfs.set_value(DEFAULT_VFS);
            app.prompt.set_value(DEFAULT_PROMPT);
            app.provider.set_value(DEFAULT_PROVIDER);
            app.model.set_value(DEFAULT_MODEL);
            app.base_url.set_value(DEFAULT_BASE_URL);
            app.api_key.set_value("");
            set_status(&app.status, "Demo reset.", Some("status-ok"));
            app.output
                .set_text_content(Some("Press “Run Agent” to start the demo."));
            app.debug.set_text_content(Some("No tool calls yet."));
            app.meta.set_text_content(Some(
                "The browser will construct a fresh loop agent for each run.",
            ));
        }));
        shared
            .reset
            .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())?;
        on_click.forget();
    }

    Ok(())
}

struct AppElements {
    vfs: HtmlTextAreaElement,
    prompt: HtmlTextAreaElement,
    provider: HtmlSelectElement,
    model: HtmlInputElement,
    base_url: HtmlInputElement,
    api_key: HtmlInputElement,
    output: HtmlElement,
    debug: HtmlElement,
    meta: HtmlElement,
    status: HtmlElement,
    run: HtmlButtonElement,
    reset: HtmlButtonElement,
    _window: Window,
}

async fn run_demo(app: Rc<AppElements>) {
    app.run.set_disabled(true);
    set_status(&app.status, "Running loop agent in wasm...", None);
    app.debug
        .set_text_content(Some("Waiting for progress events..."));

    let vfs_value = app.vfs.value();
    let prompt = app.prompt.value();
    let provider_name = app.provider.value();
    let model = app.model.value();
    let base_url = app.base_url.value();
    let api_key = app.api_key.value();

    let result = async {
        let files = parse_virtual_files(&vfs_value)?;
        let workspace_fs = Arc::new(MemoryFileSystem::from_iter(files));
        let http_client = Arc::new(BrowserHttpClient);
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        providers.insert(
            provider_name.clone(),
            build_provider(&provider_name, &api_key, &base_url)?,
        );
        let debug_events = Arc::new(Mutex::new(Vec::new()));

        let mut options = LoopAgentOptions::new(
            provider_name.clone(),
            model,
            "browser-demo",
            ".",
        );
        options.max_turns = 10;

        let agent = LoopAgent::new(
            http_client,
            providers,
            workspace_fs,
            options,
        )?;

        let response = agent
            .send_input_with_progress(&prompt, {
                let debug_events = debug_events.clone();
                move |event| {
                    debug_events
                        .lock()
                        .expect("debug events lock")
                        .push(format_progress_event(&event));
                }
            })
            .await?;
        let debug_log = debug_events.lock().expect("debug events lock").join("\n\n");
        Ok::<_, zagent_core::Error>((response, debug_log))
    }
    .await;

    match result {
        Ok((response, debug_log)) => {
            app.output.set_text_content(Some(&response.output));
            app.debug.set_text_content(Some(&debug_log));
            app.meta.set_text_content(Some(&format!(
                "Turns: {} | Tool calls: {} | Prompt tokens: {} | Completion tokens: {}",
                response.turns,
                response.tool_calls,
                response.prompt_tokens,
                response.completion_tokens
            )));
            set_status(
                &app.status,
                "Completed in-browser using the injected JSON VFS and configured provider.",
                Some("status-ok"),
            );
        }
        Err(err) => {
            app.output
                .set_text_content(Some(&format!("Run failed:\n{err}")));
            app.debug.set_text_content(Some(
                "Run failed before the debug stream completed. Check the status line and VFS JSON.",
            ));
            app.meta.set_text_content(Some(
                "If this is a live-provider run, also check CORS, model name, base URL, and API key.",
            ));
            set_status(
                &app.status,
                &format!("Run failed: {err}"),
                Some("status-error"),
            );
        }
    }

    app.run.set_disabled(false);
}

fn parse_virtual_files(raw: &str) -> zagent_core::Result<Vec<(String, String)>> {
    let value: Value = serde_json::from_str(raw)?;
    let object = value
        .as_object()
        .ok_or_else(|| zagent_core::Error::custom("VFS input must be a JSON object"))?;

    let mut files = Vec::with_capacity(object.len());
    for (path, content) in object {
        let content = content.as_str().ok_or_else(|| {
            zagent_core::Error::custom(format!("VFS entry '{path}' must be a string"))
        })?;
        files.push((path.clone(), content.to_string()));
    }

    if files.is_empty() {
        return Err(zagent_core::Error::custom(
            "VFS input must define at least one file",
        ));
    }

    Ok(files)
}

fn build_provider(
    provider_name: &str,
    api_key: &str,
    base_url: &str,
) -> zagent_core::Result<Arc<dyn Provider>> {
    match provider_name {
        "openai" => {
            if api_key.trim().is_empty() {
                return Err(zagent_core::Error::config(
                    "OpenAI requires an API key in the browser demo",
                ));
            }
            let provider = if base_url.trim().is_empty() {
                OpenAiProvider::new_api_key(api_key)
            } else {
                OpenAiProvider::new_api_key(api_key).with_base_url(base_url)
            };
            Ok(Arc::new(provider))
        }
        "openrouter" => {
            if api_key.trim().is_empty() {
                return Err(zagent_core::Error::config(
                    "OpenRouter requires an API key in the browser demo",
                ));
            }
            let provider = if base_url.trim().is_empty() {
                OpenRouterProvider::new(api_key)
            } else {
                OpenRouterProvider::new(api_key).with_base_url(base_url)
            };
            Ok(Arc::new(provider))
        }
        "local" => {
            if base_url.trim().is_empty() {
                return Err(zagent_core::Error::config(
                    "Local provider requires a base URL such as http://127.0.0.1:1234/v1",
                ));
            }
            let provider = if api_key.trim().is_empty() {
                LocalProvider::new(base_url)
            } else {
                LocalProvider::new(base_url).with_api_key(api_key)
            };
            Ok(Arc::new(provider))
        }
        other => Err(zagent_core::Error::config(format!(
            "Unsupported provider '{other}'"
        ))),
    }
}

fn format_progress_event(event: &AgentProgressEvent) -> String {
    match event {
        AgentProgressEvent::ModelRequestStarted {
            turn,
            provider,
            model,
            message_count,
            tool_count,
            ..
        } => format!(
            "model request\nturn: {turn}\nprovider: {provider}\nmodel: {model}\nmessages: {message_count}\ntools visible: {tool_count}"
        ),
        AgentProgressEvent::ModelResponseReceived {
            turn,
            finish_reason,
            latency_ms,
            prompt_tokens,
            completion_tokens,
            ..
        } => format!(
            "model response\nturn: {turn}\nfinish_reason: {}\nlatency_ms: {latency_ms}\nprompt_tokens: {}\ncompletion_tokens: {}",
            finish_reason.as_deref().unwrap_or("unknown"),
            prompt_tokens
                .map(|v| v.to_string())
                .as_deref()
                .unwrap_or("n/a"),
            completion_tokens
                .map(|v| v.to_string())
                .as_deref()
                .unwrap_or("n/a")
        ),
        AgentProgressEvent::ToolCallStarted {
            tool_call_id,
            tool_name,
            arguments,
            ..
        } => format!("tool started\nid: {tool_call_id}\nname: {tool_name}\narguments: {arguments}"),
        AgentProgressEvent::ToolCallFinished {
            tool_call_id,
            tool_name,
            success,
            latency_ms,
            result,
            ..
        } => format!(
            "tool finished\nid: {tool_call_id}\nname: {tool_name}\nsuccess: {success}\nlatency_ms: {latency_ms}\nresult:\n{}",
            truncate_block(result, 1200)
        ),
    }
}

fn truncate_block(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("\n[truncated]");
            break;
        }
        out.push(ch);
    }
    out
}

fn window() -> Result<Window, JsValue> {
    web_sys::window().ok_or_else(|| JsValue::from_str("missing window"))
}

fn document() -> Result<Document, JsValue> {
    window()?
        .document()
        .ok_or_else(|| JsValue::from_str("missing document"))
}

fn textarea(document: &Document, id: &str) -> Result<HtmlTextAreaElement, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing textarea #{id}")))?
        .dyn_into::<HtmlTextAreaElement>()
        .map_err(|_| JsValue::from_str(&format!("element #{id} is not a textarea")))
}

fn input(document: &Document, id: &str) -> Result<HtmlInputElement, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing input #{id}")))?
        .dyn_into::<HtmlInputElement>()
        .map_err(|_| JsValue::from_str(&format!("element #{id} is not an input")))
}

fn select(document: &Document, id: &str) -> Result<HtmlSelectElement, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing select #{id}")))?
        .dyn_into::<HtmlSelectElement>()
        .map_err(|_| JsValue::from_str(&format!("element #{id} is not a select")))
}

fn button(document: &Document, id: &str) -> Result<HtmlButtonElement, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing button #{id}")))?
        .dyn_into::<HtmlButtonElement>()
        .map_err(|_| JsValue::from_str(&format!("element #{id} is not a button")))
}

fn element(document: &Document, id: &str) -> Result<HtmlElement, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing element #{id}")))?
        .dyn_into::<HtmlElement>()
        .map_err(|_| JsValue::from_str(&format!("element #{id} is not an HtmlElement")))
}

fn set_status(element: &HtmlElement, text: &str, class_name: Option<&str>) {
    element.set_text_content(Some(text));
    let _ = set_status_class(element, class_name);
}

fn set_status_class(element: &HtmlElement, class_name: Option<&str>) -> Result<(), JsValue> {
    element.class_list().remove_2("status-ok", "status-error")?;
    if let Some(class_name) = class_name {
        element.class_list().add_1(class_name)?;
    }
    Ok(())
}
