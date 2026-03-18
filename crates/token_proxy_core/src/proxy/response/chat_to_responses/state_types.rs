// Small helper types extracted to keep `chat_to_responses.rs` under the project's line limit.

use serde_json::Value;

pub(super) struct ReasoningOutput {
    pub(super) id: String,
    pub(super) output_index: u64,
    pub(super) text: String,
    pub(super) encrypted_content: Option<String>,
}

pub(super) struct MessageOutput {
    pub(super) id: String,
    pub(super) output_index: u64,
    pub(super) text: String,
    pub(super) text_part_started: bool,
    pub(super) audio: Option<Value>,
}

pub(super) struct FunctionCallOutput {
    pub(super) id: String,
    pub(super) output_index: u64,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) arguments: String,
}
