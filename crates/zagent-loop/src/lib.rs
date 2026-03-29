mod agent;
mod fs;
mod session;
mod tools;

pub use agent::{LoopAgent, LoopAgentOptions, LoopAgentResponse};
pub use fs::{HostFileSystem, MemoryFileSystem};
pub use session::InMemorySessionStore;
pub use tools::{
	FileEditTool, FileReadTool, FileWriteTool, ListDirTool, build_file_tools,
	register_file_tools,
};
pub use zagent_core::agent::{AgentProgressEvent, ContextManagementPolicy};

pub type EmbeddedAgent = LoopAgent;
pub type EmbeddedAgentOptions = LoopAgentOptions;
pub type EmbeddedAgentResponse = LoopAgentResponse;

pub use tools::build_file_tools as build_embedded_tools;
