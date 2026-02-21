#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentNodeView {
    pub id: usize,
    pub name: String,
    pub depth: u32,
    pub phase_badge: String,
    pub tokens_sent: u64,
    pub tokens_received: u64,
    pub cached_tokens: u64,
    pub context_pct: f64,
    pub cost_usd: f64,
    pub reasoning_text: Option<String>,
    pub handoff_message: Option<String>,
    pub handoff_context: Option<String>,
    pub last_event: String,
    pub timeline: Vec<AgentTimelineEntryView>,
    pub children: Vec<AgentNodeView>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentTimelineEntryView {
    pub text: String,
    pub kind: String,
    pub phase: String,
    pub sequence: Option<u64>,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatTurnView {
    pub id: usize,
    pub author: String,
    pub role_class: String,
    pub markdown: String,
    pub agent_roots: Vec<AgentNodeView>,
    pub agent_details_collapsed: bool,
    pub agent_details_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FooterTotalsView {
    pub top_agent: String,
    pub tokens_sent: u64,
    pub tokens_received: u64,
    pub cached_tokens: u64,
    pub context_pct: f64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ServerTabView {
    pub id: usize,
    pub label: String,
    pub connected: bool,
}
