use crate::provider::types::Message;

/// Manages conversation state and provides utilities for context window management.
pub struct Conversation {
    pub messages: Vec<Message>,
    pub max_context_messages: Option<usize>,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            max_context_messages: None,
        }
    }

    pub fn with_max_context(mut self, max: usize) -> Self {
        self.max_context_messages = Some(max);
        self
    }

    pub fn add(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Get messages for the next API call, respecting context window limits.
    /// Always keeps the system prompt as first message.
    pub fn get_context_messages(&self, system_prompt: &str) -> Vec<Message> {
        let mut result = vec![Message::system(system_prompt)];

        if let Some(max) = self.max_context_messages {
            let msgs = if self.messages.len() > max {
                &self.messages[self.messages.len() - max..]
            } else {
                &self.messages
            };
            result.extend(msgs.iter().cloned());
        } else {
            result.extend(self.messages.iter().cloned());
        }

        result
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}
