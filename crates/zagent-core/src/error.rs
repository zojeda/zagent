use derive_more::{Display, From};

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug, Display, From)]
pub enum Error {
    // -- Generic
    #[from(String, &String, &str)]
    Custom(String),

    // -- Serialization
    #[display("JSON error: {_0}")]
    #[from]
    Json(serde_json::Error),

    // -- I/O
    #[display("I/O error: {_0}")]
    #[from]
    Io(std::io::Error),

    // -- API errors
    #[display("API error (HTTP {status}): {message}")]
    Api { status: u16, message: String },

    // -- Provider errors
    #[display("Provider error ({provider}): {message}")]
    Provider { provider: String, message: String },

    // -- Tool execution errors
    #[display("Tool '{tool}' failed: {message}")]
    ToolExecution { tool: String, message: String },

    // -- Session errors
    #[display("Session error: {_0}")]
    Session(String),

    // -- Config errors
    #[display("Config error: {_0}")]
    Config(String),
}

impl Error {
    pub fn custom(val: impl Into<String>) -> Self {
        Self::Custom(val.into())
    }

    pub fn api(status: u16, message: impl Into<String>) -> Self {
        Self::Api {
            status,
            message: message.into(),
        }
    }

    pub fn provider(provider: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Provider {
            provider: provider.into(),
            message: message.into(),
        }
    }

    pub fn tool(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ToolExecution {
            tool: tool.into(),
            message: message.into(),
        }
    }

    pub fn session(message: impl Into<String>) -> Self {
        Self::Session(message.into())
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }
}

impl std::error::Error for Error {}
