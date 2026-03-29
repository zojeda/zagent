use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellStreamChunk {
    pub stream_id: String,
    pub channel: String,
    pub text: String,
}

fn shell_stream_tx() -> &'static broadcast::Sender<ShellStreamChunk> {
    static TX: OnceLock<broadcast::Sender<ShellStreamChunk>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, _) = broadcast::channel(4096);
        tx
    })
}

pub fn publish(chunk: ShellStreamChunk) {
    let _ = shell_stream_tx().send(chunk);
}

pub fn subscribe() -> broadcast::Receiver<ShellStreamChunk> {
    shell_stream_tx().subscribe()
}
