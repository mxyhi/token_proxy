use axum::http::HeaderMap;
use serde_json::{Map, Value};

use super::constants::KIRO_AGENTIC_SYSTEM_PROMPT;
use super::tools::convert_openai_tools;
use super::types::{
    KiroConversationState, KiroCurrentMessage, KiroPayload, KiroUserInputMessage,
    KiroUserInputMessageContext,
};
use super::utils::random_uuid;
use inference::build_inference_config;
use input::extract_input_messages;
use messages::{build_final_content, deduplicate_tool_results, process_messages};
use system::{
    extract_response_format_hint, extract_system_prompt, extract_tool_choice_hint, inject_hint,
    inject_timestamp, is_thinking_enabled,
};

mod claude;
mod inference;
mod input;
mod messages;
mod system;

const THINKING_HINT: &str =
    "<thinking_mode>enabled</thinking_mode>\n<max_thinking_length>200000</max_thinking_length>";

pub(crate) struct BuildPayloadResult {
    pub(crate) payload: Vec<u8>,
}

pub(crate) use claude::build_payload_from_claude;

pub(crate) fn build_payload_from_responses(
    request: &Value,
    model_id: &str,
    profile_arn: Option<&str>,
    origin: &str,
    is_agentic: bool,
    is_chat_only: bool,
    headers: &HeaderMap,
) -> Result<BuildPayloadResult, String> {
    let object = request
        .as_object()
        .ok_or_else(|| "Request body must be a JSON object.".to_string())?;

    let messages = extract_input_messages(object)?;
    let system_prompt = prepare_system_prompt(object, &messages, headers, is_agentic);

    let (history, current_user, current_tool_results) =
        process_messages(&messages, model_id, origin);
    let current_message = build_current_message(
        &history,
        current_user,
        current_tool_results,
        model_id,
        origin,
        &system_prompt,
        object,
        is_chat_only,
    );

    let payload = KiroPayload {
        conversation_state: KiroConversationState {
            chat_trigger_type: "MANUAL".to_string(),
            conversation_id: random_uuid(),
            current_message,
            history,
        },
        profile_arn: profile_arn.map(|value| value.to_string()),
        inference_config: build_inference_config(object),
    };

    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|err| format!("Failed to serialize request payload: {err}"))?;

    Ok(BuildPayloadResult {
        payload: payload_bytes,
    })
}

fn prepare_system_prompt(
    object: &Map<String, Value>,
    messages: &[Value],
    headers: &HeaderMap,
    is_agentic: bool,
) -> String {
    let mut system_prompt = extract_system_prompt(object, messages);
    let thinking_enabled = is_thinking_enabled(object, headers, &system_prompt);
    if thinking_enabled && !system::has_thinking_tags(&system_prompt) {
        system_prompt = inject_hint(system_prompt, THINKING_HINT);
    }
    system_prompt = inject_timestamp(system_prompt);
    if is_agentic {
        system_prompt = inject_hint(system_prompt, KIRO_AGENTIC_SYSTEM_PROMPT.trim());
    }

    if let Some(tool_choice_hint) = extract_tool_choice_hint(object) {
        system_prompt = inject_hint(system_prompt, &tool_choice_hint);
    }
    if let Some(response_format_hint) = extract_response_format_hint(object) {
        system_prompt = inject_hint(system_prompt, &response_format_hint);
    }

    system_prompt
}

fn build_current_message(
    history: &[super::types::KiroHistoryMessage],
    current_user: Option<KiroUserInputMessage>,
    mut tool_results: Vec<super::types::KiroToolResult>,
    model_id: &str,
    origin: &str,
    system_prompt: &str,
    object: &Map<String, Value>,
    is_chat_only: bool,
) -> KiroCurrentMessage {
    if let Some(mut user) = current_user {
        let prompt = if history.is_empty() {
            system_prompt
        } else {
            ""
        };
        user.content = build_final_content(&user.content, prompt, &tool_results);
        tool_results = deduplicate_tool_results(tool_results);
        let tools = convert_openai_tools(object.get("tools"), is_chat_only);
        if !tools.is_empty() || !tool_results.is_empty() {
            user.user_input_message_context = Some(KiroUserInputMessageContext {
                tool_results,
                tools,
            });
        }
        return KiroCurrentMessage {
            user_input_message: user,
        };
    }

    let fallback = if system_prompt.trim().is_empty() {
        "Continue".to_string()
    } else {
        format!("--- SYSTEM PROMPT ---\n{system_prompt}\n--- END SYSTEM PROMPT ---\n")
    };
    KiroCurrentMessage {
        user_input_message: KiroUserInputMessage {
            content: fallback,
            model_id: model_id.to_string(),
            origin: origin.to_string(),
            images: Vec::new(),
            user_input_message_context: None,
        },
    }
}
