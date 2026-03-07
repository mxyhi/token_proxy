// Small helper types extracted to keep `chat_to_responses.rs` under the project's line limit.

pub(super) struct MessageOutput {
    pub(super) id: String,
    pub(super) output_index: u64,
    pub(super) text: String,
}

pub(super) struct FunctionCallOutput {
    pub(super) id: String,
    pub(super) output_index: u64,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) arguments: String,
}
