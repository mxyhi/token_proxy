//! Pure protocol contracts and transformations shared by every runtime adapter.
//!
//! Network transport, persistence, account state, and application config do
//! not belong here. Functions consume values or byte slices and return values.

pub mod anthropic_tools;
pub mod codex_tool_types;
pub mod compat_content;
pub mod compat_reason;
pub mod gemini_tools;
pub mod gemini_usage;
pub mod openai_usage;
pub mod request_token_estimate;
pub mod responses_annotations;
pub mod responses_error;
pub mod responses_failure;
pub mod responses_input;
pub mod responses_sequence;
pub mod responses_text;
pub mod schema_bool;
pub mod sse;
pub mod token_estimator;
pub mod tool_identity;
pub mod xai_client_tools;
pub mod xai_forbidden;
