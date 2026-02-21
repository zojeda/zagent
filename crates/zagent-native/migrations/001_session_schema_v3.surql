-- zAgent session storage schema v3 (SurrealDB)
-- Notes:
-- - `sessions.payload_json` is legacy and intentionally not part of the schemafull fields.
-- - app-layer migration rewrites legacy rows into schemafull columns.

DEFINE TABLE IF NOT EXISTS providers SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS provider_id ON TABLE providers TYPE string;
DEFINE FIELD IF NOT EXISTS name ON TABLE providers TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE providers TYPE datetime;
DEFINE FIELD IF NOT EXISTS updated_at ON TABLE providers TYPE datetime;
DEFINE INDEX IF NOT EXISTS providers_name_uq ON TABLE providers FIELDS name UNIQUE;

DEFINE TABLE IF NOT EXISTS models SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS model_id ON TABLE models TYPE string;
DEFINE FIELD IF NOT EXISTS name ON TABLE models TYPE string;
DEFINE FIELD IF NOT EXISTS provider_name ON TABLE models TYPE string;
DEFINE FIELD IF NOT EXISTS provider_ref ON TABLE models TYPE record<providers> REFERENCE ON DELETE CASCADE;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE models TYPE datetime;
DEFINE FIELD IF NOT EXISTS updated_at ON TABLE models TYPE datetime;
DEFINE INDEX IF NOT EXISTS models_provider_name_uq ON TABLE models FIELDS provider_name, name UNIQUE;

DEFINE TABLE IF NOT EXISTS sessions SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS session_id ON TABLE sessions TYPE string;
DEFINE FIELD IF NOT EXISTS name ON TABLE sessions TYPE string;
DEFINE FIELD IF NOT EXISTS provider ON TABLE sessions TYPE string;
DEFINE FIELD IF NOT EXISTS provider_ref ON TABLE sessions TYPE option<record<providers>> REFERENCE ON DELETE UNSET;
DEFINE FIELD IF NOT EXISTS model ON TABLE sessions TYPE string;
DEFINE FIELD IF NOT EXISTS model_ref ON TABLE sessions TYPE option<record<models>> REFERENCE ON DELETE UNSET;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE sessions TYPE datetime;
DEFINE FIELD IF NOT EXISTS updated_at ON TABLE sessions TYPE datetime;
DEFINE FIELD IF NOT EXISTS message_count ON TABLE sessions TYPE int;
DEFINE FIELD IF NOT EXISTS total_prompt_tokens ON TABLE sessions TYPE int;
DEFINE FIELD IF NOT EXISTS total_completion_tokens ON TABLE sessions TYPE int;
DEFINE FIELD IF NOT EXISTS total_tokens ON TABLE sessions VALUE <int>total_prompt_tokens + <int>total_completion_tokens;
DEFINE FIELD IF NOT EXISTS system_prompt ON TABLE sessions TYPE string;
DEFINE FIELD IF NOT EXISTS working_dir ON TABLE sessions TYPE string;
DEFINE FIELD messages ON TABLE sessions TYPE array<object> FLEXIBLE;
DEFINE FIELD tool_executions ON TABLE sessions TYPE array<object> FLEXIBLE;
DEFINE INDEX IF NOT EXISTS sessions_session_id_uq ON TABLE sessions FIELDS session_id UNIQUE;
DEFINE INDEX IF NOT EXISTS sessions_updated_at_idx ON TABLE sessions FIELDS updated_at;

DEFINE TABLE IF NOT EXISTS session_events SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS event_id ON TABLE session_events TYPE string;
DEFINE FIELD IF NOT EXISTS session_id ON TABLE session_events TYPE string;
DEFINE FIELD IF NOT EXISTS session_ref ON TABLE session_events TYPE record<sessions> REFERENCE ON DELETE CASCADE;
DEFINE FIELD IF NOT EXISTS sequence ON TABLE session_events TYPE option<int>;
DEFINE FIELD IF NOT EXISTS parent_event_id ON TABLE session_events TYPE option<string>;
DEFINE FIELD IF NOT EXISTS parent_event_ref ON TABLE session_events TYPE option<record<session_events>> REFERENCE ON DELETE UNSET;
DEFINE FIELD IF NOT EXISTS kind ON TABLE session_events TYPE string;
DEFINE FIELD IF NOT EXISTS phase ON TABLE session_events TYPE option<string>;
DEFINE FIELD IF NOT EXISTS agent ON TABLE session_events TYPE string;
DEFINE FIELD IF NOT EXISTS handoff_depth ON TABLE session_events TYPE int;
DEFINE FIELD IF NOT EXISTS turn ON TABLE session_events TYPE option<int>;
DEFINE FIELD IF NOT EXISTS provider ON TABLE session_events TYPE option<string>;
DEFINE FIELD IF NOT EXISTS provider_ref ON TABLE session_events TYPE option<record<providers>> REFERENCE ON DELETE UNSET;
DEFINE FIELD IF NOT EXISTS model ON TABLE session_events TYPE option<string>;
DEFINE FIELD IF NOT EXISTS model_ref ON TABLE session_events TYPE option<record<models>> REFERENCE ON DELETE UNSET;
DEFINE FIELD IF NOT EXISTS tool_name ON TABLE session_events TYPE option<string>;
DEFINE FIELD IF NOT EXISTS success ON TABLE session_events TYPE option<bool>;
DEFINE FIELD IF NOT EXISTS finish_reason ON TABLE session_events TYPE option<string>;
DEFINE FIELD IF NOT EXISTS latency_ms ON TABLE session_events TYPE option<int>;
DEFINE FIELD IF NOT EXISTS prompt_tokens ON TABLE session_events TYPE option<int>;
DEFINE FIELD IF NOT EXISTS completion_tokens ON TABLE session_events TYPE option<int>;
DEFINE FIELD IF NOT EXISTS total_tokens ON TABLE session_events TYPE option<int>;
DEFINE FIELD IF NOT EXISTS cached_prompt_tokens ON TABLE session_events TYPE option<int>;
DEFINE FIELD IF NOT EXISTS effective_prompt_tokens ON TABLE session_events VALUE (IF prompt_tokens = NONE THEN 0 ELSE <int>prompt_tokens END) - (IF cached_prompt_tokens = NONE THEN 0 ELSE <int>cached_prompt_tokens END);
DEFINE FIELD IF NOT EXISTS cost_usd ON TABLE session_events TYPE option<float>;
DEFINE FIELD IF NOT EXISTS credits_remaining ON TABLE session_events TYPE option<float>;
DEFINE FIELD IF NOT EXISTS arguments ON TABLE session_events TYPE option<string>;
DEFINE FIELD IF NOT EXISTS result ON TABLE session_events TYPE option<string>;
DEFINE FIELD IF NOT EXISTS payload ON TABLE session_events TYPE object FLEXIBLE;
DEFINE FIELD IF NOT EXISTS created_at ON TABLE session_events TYPE datetime;
DEFINE INDEX IF NOT EXISTS session_events_event_id_uq ON TABLE session_events FIELDS event_id UNIQUE;
DEFINE INDEX IF NOT EXISTS session_events_session_seq_idx ON TABLE session_events FIELDS session_ref, sequence;
DEFINE INDEX IF NOT EXISTS session_events_session_created_idx ON TABLE session_events FIELDS session_ref, created_at;
DEFINE INDEX IF NOT EXISTS session_events_parent_idx ON TABLE session_events FIELDS parent_event_ref;
DEFINE INDEX IF NOT EXISTS session_events_provider_idx ON TABLE session_events FIELDS provider_ref;
DEFINE INDEX IF NOT EXISTS session_events_model_idx ON TABLE session_events FIELDS model_ref;
