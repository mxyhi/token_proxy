use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroPayload {
    pub(crate) conversation_state: KiroConversationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) profile_arn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) inference_config: Option<KiroInferenceConfig>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroInferenceConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) top_p: Option<f64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroConversationState {
    pub(crate) chat_trigger_type: String,
    pub(crate) conversation_id: String,
    pub(crate) current_message: KiroCurrentMessage,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) history: Vec<KiroHistoryMessage>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroCurrentMessage {
    pub(crate) user_input_message: KiroUserInputMessage,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroHistoryMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user_input_message: Option<KiroUserInputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) assistant_response_message: Option<KiroAssistantResponseMessage>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroImage {
    pub(crate) format: String,
    pub(crate) source: KiroImageSource,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroImageSource {
    pub(crate) bytes: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroUserInputMessage {
    pub(crate) content: String,
    pub(crate) model_id: String,
    pub(crate) origin: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) images: Vec<KiroImage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user_input_message_context: Option<KiroUserInputMessageContext>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroUserInputMessageContext {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tool_results: Vec<KiroToolResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tools: Vec<KiroToolWrapper>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroToolResult {
    pub(crate) content: Vec<KiroTextContent>,
    pub(crate) status: String,
    pub(crate) tool_use_id: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroTextContent {
    pub(crate) text: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroToolWrapper {
    pub(crate) tool_specification: KiroToolSpecification,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroToolSpecification {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_schema: KiroInputSchema,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroInputSchema {
    pub(crate) json: Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroAssistantResponseMessage {
    pub(crate) content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tool_uses: Vec<KiroToolUse>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiroToolUse {
    pub(crate) tool_use_id: String,
    pub(crate) name: String,
    pub(crate) input: Map<String, Value>,
}
