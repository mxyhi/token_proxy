use axum::body::Bytes;
use serde_json::{json, Map, Value};
use token_proxy_protocol::compat_reason::SummaryVisibility;

use super::{
    anthropic_compat, codex_compat, compat_content, compat_reason, gemini_compat,
    http_client::ProxyHttpClients, model,
};

mod extract;
pub(crate) mod images;
mod input;
mod message;
mod tools;
mod usage {
    pub(crate) use token_proxy_protocol::openai_usage::*;
}
pub(crate) use usage::{map_usage_chat_to_responses, map_usage_responses_to_chat};

pub(crate) const CHAT_PATH: &str = "/v1/chat/completions";
pub(crate) const RESPONSES_PATH: &str = "/v1/responses";

pub(crate) const PROVIDER_CHAT: &str = "openai";
pub(crate) const PROVIDER_RESPONSES: &str = "openai-response";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApiFormat {
    ChatCompletions,
    Responses,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormatTransform {
    None,
    ChatToResponses,
    ResponsesToChat,
    ResponsesToAnthropic,
    AnthropicToResponses,
    AnthropicCountTokensToResponsesInputTokens,
    ResponsesInputTokensToAnthropicCountTokens,
    AnthropicToCodex,
    ChatToAnthropic,
    AnthropicToChat,
    GeminiToAnthropic,
    AnthropicToGemini,
    ChatToGemini,
    GeminiToChat,
    ResponsesToGemini,
    GeminiToResponses,
    KiroToAnthropic,
    ChatToCodex,
    ResponsesToCodex,
    ImagesGenerationsToCodex,
    CodexToChat,
    CodexToResponses,
    CodexToImagesGenerations,
    CodexToAnthropic,
}

pub(crate) fn inbound_format(path: &str) -> Option<ApiFormat> {
    let path = strip_query(path);
    match path {
        CHAT_PATH => Some(ApiFormat::ChatCompletions),
        RESPONSES_PATH => Some(ApiFormat::Responses),
        _ => None,
    }
}

fn strip_query(path: &str) -> &str {
    path.split_once('?').map(|(path, _)| path).unwrap_or(path)
}

#[cfg(test)]
pub(crate) async fn transform_request_body(
    transform: FormatTransform,
    body: &Bytes,
    http_clients: &ProxyHttpClients,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    transform_request_body_with_prompt_cache_key(transform, body, http_clients, model_hint, None)
        .await
}

pub(crate) async fn transform_request_body_with_prompt_cache_key(
    transform: FormatTransform,
    body: &Bytes,
    http_clients: &ProxyHttpClients,
    model_hint: Option<&str>,
    prompt_cache_key: Option<&str>,
) -> Result<Bytes, String> {
    match transform {
        FormatTransform::None => Ok(body.clone()),
        FormatTransform::ChatToResponses => {
            chat_request_to_responses_with_prompt_cache_key(body, prompt_cache_key)
        }
        FormatTransform::ResponsesToChat => responses_request_to_chat(body),
        FormatTransform::ResponsesToAnthropic => {
            anthropic_compat::responses_request_to_anthropic(body, http_clients).await
        }
        FormatTransform::AnthropicToResponses => {
            anthropic_compat::anthropic_request_to_responses(body, http_clients).await
        }
        FormatTransform::AnthropicCountTokensToResponsesInputTokens => {
            anthropic_count_tokens_request_to_responses_input_tokens(body, http_clients).await
        }
        FormatTransform::AnthropicToCodex => {
            let intermediate =
                anthropic_compat::anthropic_request_to_responses(body, http_clients).await?;
            codex_compat::responses_request_to_codex_with_prompt_cache_key(
                &intermediate,
                model_hint,
                prompt_cache_key,
            )
        }
        FormatTransform::ChatToAnthropic => {
            let intermediate = chat_request_to_responses(body)?;
            anthropic_compat::responses_request_to_anthropic(&intermediate, http_clients).await
        }
        FormatTransform::AnthropicToChat => {
            let intermediate =
                anthropic_compat::anthropic_request_to_responses(body, http_clients).await?;
            let chat = responses_request_to_chat(&intermediate)?;
            preserve_anthropic_max_tokens_for_chat(body, &chat)
        }
        FormatTransform::GeminiToAnthropic => {
            gemini_request_to_anthropic(body, http_clients, model_hint).await
        }
        FormatTransform::AnthropicToGemini => anthropic_request_to_gemini(body, http_clients).await,
        FormatTransform::ChatToGemini => gemini_compat::chat_request_to_gemini(body),
        FormatTransform::GeminiToChat => gemini_compat::gemini_request_to_chat(body, model_hint),
        FormatTransform::ResponsesToGemini => responses_request_to_gemini(body),
        FormatTransform::GeminiToResponses => gemini_request_to_responses(body, model_hint),
        FormatTransform::KiroToAnthropic => Ok(body.clone()),
        FormatTransform::ChatToCodex => codex_compat::chat_request_to_codex_with_prompt_cache_key(
            body,
            model_hint,
            prompt_cache_key,
        ),
        FormatTransform::ResponsesToCodex => {
            codex_compat::responses_request_to_codex_with_prompt_cache_key(
                body,
                model_hint,
                prompt_cache_key,
            )
        }
        FormatTransform::ImagesGenerationsToCodex => {
            let responses_body = images::images_generation_request_to_responses(body)?;
            codex_compat::responses_request_to_codex_with_prompt_cache_key(
                &responses_body,
                None,
                prompt_cache_key,
            )
        }
        FormatTransform::CodexToChat
        | FormatTransform::CodexToResponses
        | FormatTransform::CodexToImagesGenerations
        | FormatTransform::CodexToAnthropic
        | FormatTransform::ResponsesInputTokensToAnthropicCountTokens => Ok(body.clone()),
    }
}

pub(crate) fn transform_response_body(
    transform: FormatTransform,
    bytes: &Bytes,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    transform_response_body_with_request_body(transform, bytes, model_hint, None)
}

pub(crate) fn transform_response_body_with_request_body(
    transform: FormatTransform,
    bytes: &Bytes,
    model_hint: Option<&str>,
    request_body: Option<&str>,
) -> Result<Bytes, String> {
    match transform {
        FormatTransform::None => Ok(bytes.clone()),
        FormatTransform::ChatToResponses => chat_response_to_responses(bytes),
        FormatTransform::ResponsesToChat => responses_response_to_chat(bytes, model_hint),
        FormatTransform::ResponsesToAnthropic => {
            anthropic_compat::responses_response_to_anthropic(bytes, model_hint)
        }
        FormatTransform::AnthropicToResponses => {
            anthropic_compat::anthropic_response_to_responses(bytes)
        }
        FormatTransform::ResponsesInputTokensToAnthropicCountTokens => {
            responses_input_tokens_response_to_anthropic_count_tokens(bytes)
        }
        FormatTransform::AnthropicToCodex => {
            Err("Codex response conversion is handled upstream.".to_string())
        }
        FormatTransform::ChatToAnthropic => {
            let intermediate = chat_response_to_responses(bytes)?;
            anthropic_compat::responses_response_to_anthropic(&intermediate, model_hint)
        }
        FormatTransform::AnthropicToChat => {
            let intermediate = anthropic_compat::anthropic_response_to_responses(bytes)?;
            responses_response_to_chat(&intermediate, model_hint)
        }
        FormatTransform::GeminiToAnthropic => gemini_response_to_anthropic(bytes, model_hint),
        FormatTransform::AnthropicToGemini => anthropic_response_to_gemini(bytes, model_hint),
        FormatTransform::ChatToGemini => gemini_compat::chat_response_to_gemini(bytes, model_hint),
        FormatTransform::GeminiToChat => gemini_compat::gemini_response_to_chat(bytes, model_hint),
        FormatTransform::ResponsesToGemini => responses_response_to_gemini(bytes, model_hint),
        FormatTransform::GeminiToResponses => {
            gemini_response_to_responses(bytes, model_hint, request_body)
        }
        FormatTransform::KiroToAnthropic => {
            Err("Kiro response conversion is handled upstream.".to_string())
        }
        FormatTransform::CodexToAnthropic => {
            let intermediate = codex_compat::codex_response_to_responses(bytes, None)?;
            anthropic_compat::responses_response_to_anthropic(&intermediate, model_hint)
        }
        FormatTransform::CodexToImagesGenerations => {
            images::codex_response_to_images_generation(bytes, None).map_err(|err| err.message)
        }
        FormatTransform::CodexToChat | FormatTransform::CodexToResponses => {
            Err("Codex response conversion is handled upstream.".to_string())
        }
        FormatTransform::ChatToCodex
        | FormatTransform::ResponsesToCodex
        | FormatTransform::ImagesGenerationsToCodex
        | FormatTransform::AnthropicCountTokensToResponsesInputTokens => {
            Err("Codex response conversion is handled upstream.".to_string())
        }
    }
}

async fn anthropic_count_tokens_request_to_responses_input_tokens(
    body: &Bytes,
    http_clients: &ProxyHttpClients,
) -> Result<Bytes, String> {
    let responses = anthropic_compat::anthropic_request_to_responses(body, http_clients).await?;
    let value: Value = serde_json::from_slice(&responses)
        .map_err(|_| "Converted count_tokens body must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Converted count_tokens body must be a JSON object.".to_string());
    };

    let mut output = Map::new();
    copy_keys(
        object,
        &mut output,
        &["model", "instructions", "input", "tools", "tool_choice"],
    );
    if !output.contains_key("model") || !output.contains_key("input") {
        return Err("Count tokens request must include model and messages.".to_string());
    }

    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize count_tokens request: {err}"))
}

fn responses_input_tokens_response_to_anthropic_count_tokens(
    bytes: &Bytes,
) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| "Response body must be JSON.".to_string())?;
    let input_tokens = value
        .get("input_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Responses input_tokens response missing input_tokens.".to_string())?;
    serde_json::to_vec(&json!({ "input_tokens": input_tokens }))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize count_tokens response: {err}"))
}

fn chat_request_to_responses(body: &Bytes) -> Result<Bytes, String> {
    chat_request_to_responses_with_prompt_cache_key(body, None)
}

fn chat_request_to_responses_with_prompt_cache_key(
    body: &Bytes,
    prompt_cache_key: Option<&str>,
) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Request body must be a JSON object.".to_string());
    };

    if is_responses_shaped_chat_request(object) {
        return responses_shaped_chat_request_to_responses(object);
    }

    let Some(messages) = object.get("messages").and_then(Value::as_array) else {
        return Err("Chat request must include messages.".to_string());
    };

    let (input, instructions) = chat_messages_to_responses_input(messages)?;

    // Responses API uses `input` (string or structured items).
    let mut output = Map::new();
    copy_key(object, &mut output, "model");
    output.insert("input".to_string(), Value::Array(input));
    if let Some(instructions) = instructions {
        output.insert("instructions".to_string(), Value::String(instructions));
    }
    copy_keys(
        object,
        &mut output,
        &[
            "stream",
            "temperature",
            "top_p",
            "stop",
            "metadata",
            "user",
            "seed",
            "parallel_tool_calls",
            "modalities",
            "audio",
            "previous_response_id",
            "include",
            "store",
            "background",
            "truncation",
            "service_tier",
            "safety_identifier",
            "prompt",
            "max_tool_calls",
            "prompt_cache_key",
            "prompt_cache_retention",
            "stream_options",
            "top_logprobs",
            "partial_images",
            "context_management",
        ],
    );
    ensure_prompt_cache_key(&mut output, prompt_cache_key);
    copy_key(object, &mut output, "text");

    strip_sampling_params_for_reasoning_responses_model(&mut output);

    if let Some(max_output_tokens) = object
        .get("max_completion_tokens")
        .or_else(|| object.get("max_tokens"))
        .and_then(Value::as_i64)
    {
        output.insert(
            "max_output_tokens".to_string(),
            Value::Number(max_output_tokens.into()),
        );
    }

    if let Some(tools) = object.get("tools") {
        output.insert(
            "tools".to_string(),
            tools::map_chat_tools_to_responses(tools),
        );
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        output.insert(
            "tool_choice".to_string(),
            tools::map_chat_tool_choice_to_responses(tool_choice),
        );
    }
    if let Some(response_format) = object.get("response_format") {
        merge_chat_response_format_into_responses_text(&mut output, response_format);
    }
    if let Some(reasoning) =
        map_chat_reasoning_effort_to_responses_reasoning(object.get("reasoning_effort"))
    {
        output.insert("reasoning".to_string(), reasoning);
    }
    if object.get("web_search_options").is_some() {
        append_responses_web_search_tool(&mut output, object.get("web_search_options"));
    }

    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn ensure_prompt_cache_key(output: &mut Map<String, Value>, prompt_cache_key: Option<&str>) {
    let existing = output
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if existing.is_some() {
        return;
    }
    if let Some(prompt_cache_key) = prompt_cache_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        output.insert(
            "prompt_cache_key".to_string(),
            Value::String(prompt_cache_key.to_string()),
        );
    }
}

fn is_responses_shaped_chat_request(object: &Map<String, Value>) -> bool {
    !object.contains_key("messages") && object.contains_key("input")
}

fn responses_shaped_chat_request_to_responses(
    object: &Map<String, Value>,
) -> Result<Bytes, String> {
    let mut output = object.clone();
    for key in [
        "prompt_cache_retention",
        "safety_identifier",
        "metadata",
        "stream_options",
    ] {
        output.remove(key);
    }
    strip_sampling_params_for_reasoning_responses_model(&mut output);

    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn strip_sampling_params_for_reasoning_responses_model(output: &mut Map<String, Value>) {
    if output
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(model::is_openai_responses_reasoning_model)
    {
        output.remove("temperature");
        output.remove("top_p");
    }
}

fn responses_request_to_chat(body: &Bytes) -> Result<Bytes, String> {
    let mut value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    let Some(object) = value.as_object_mut() else {
        return Err("Request body must be a JSON object.".to_string());
    };
    token_proxy_protocol::tool_identity::flatten_responses_namespaces(object, &[])?;

    let mut messages = match object.get("input") {
        Some(Value::String(text)) => vec![json!({ "role": "user", "content": text })],
        Some(Value::Array(items)) => input::responses_input_to_chat_messages(items)?,
        _ => return Err("Responses request must include input.".to_string()),
    };

    // Responses API supports `instructions`; translate it to a system message.
    if let Some(instructions) = object.get("instructions").and_then(Value::as_str) {
        if !instructions.trim().is_empty() {
            messages.insert(0, json!({ "role": "system", "content": instructions }));
        }
    }

    let mut output = Map::new();
    copy_key(object, &mut output, "model");
    output.insert("messages".to_string(), Value::Array(messages));
    copy_keys(
        object,
        &mut output,
        &[
            "stream",
            "temperature",
            "top_p",
            "stop",
            "metadata",
            "user",
            "seed",
            "parallel_tool_calls",
            "modalities",
            "audio",
            "service_tier",
            "context_management",
        ],
    );

    if let Some(max_output_tokens) = object.get("max_output_tokens").and_then(Value::as_i64) {
        // Prefer the modern chat parameter.
        output.insert(
            "max_completion_tokens".to_string(),
            Value::Number(max_output_tokens.into()),
        );
    }

    if let Some(tools) = object.get("tools") {
        let (mapped_tools, web_search_options) = tools::split_responses_tools_for_chat(tools);
        if !mapped_tools.is_empty() {
            output.insert("tools".to_string(), Value::Array(mapped_tools));
        }
        if let Some(web_search_options) = web_search_options {
            output.insert("web_search_options".to_string(), web_search_options);
        }
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        output.insert(
            "tool_choice".to_string(),
            tools::map_responses_tool_choice_to_chat(tool_choice),
        );
    }
    if let Some(response_format) = map_responses_text_to_chat_response_format(object.get("text")) {
        output.insert("response_format".to_string(), response_format);
    }
    if let Some(reasoning_effort) =
        map_responses_reasoning_to_chat_reasoning_effort(object.get("reasoning"))
    {
        output.insert("reasoning_effort".to_string(), reasoning_effort);
    }

    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn responses_request_to_gemini(body: &Bytes) -> Result<Bytes, String> {
    let visibility = responses_summary_visibility(body)?;
    let intermediate = responses_request_to_chat(body)?;
    gemini_compat::chat_request_to_gemini_with_summary_visibility(&intermediate, visibility)
}

fn gemini_request_to_responses(body: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let (intermediate, visibility) =
        gemini_compat::gemini_request_to_chat_with_summary_visibility(body, model_hint)?;
    let responses = chat_request_to_responses(&intermediate)?;
    apply_summary_visibility_to_responses(&responses, visibility)
}

fn responses_response_to_gemini(bytes: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let intermediate = responses_response_to_chat(bytes, model_hint)?;
    gemini_compat::chat_response_to_gemini(&intermediate, model_hint)
}

fn gemini_response_to_responses(
    bytes: &Bytes,
    model_hint: Option<&str>,
    request_body: Option<&str>,
) -> Result<Bytes, String> {
    let intermediate = gemini_compat::gemini_response_to_chat(bytes, model_hint)?;
    let mut output = chat_response_to_responses(&intermediate)?;
    if let Some(request_body) = request_body {
        let request = serde_json::from_str::<Value>(request_body)
            .map_err(|_| "Original Responses request must be JSON.".to_string())?;
        let mut value: Value = serde_json::from_slice(&output)
            .map_err(|_| "Converted Responses response must be JSON.".to_string())?;
        token_proxy_protocol::tool_identity::restore_responses_tool_identities(
            &mut value, &request,
        );
        output = serde_json::to_vec(&value)
            .map(Bytes::from)
            .map_err(|err| format!("Failed to serialize restored Responses response: {err}"))?;
    }
    Ok(output)
}

async fn gemini_request_to_anthropic(
    body: &Bytes,
    http_clients: &ProxyHttpClients,
    model_hint: Option<&str>,
) -> Result<Bytes, String> {
    let (intermediate, visibility) =
        gemini_compat::gemini_request_to_chat_with_summary_visibility(body, model_hint)?;
    let intermediate = chat_request_to_responses(&intermediate)?;
    let intermediate = apply_summary_visibility_to_responses(&intermediate, visibility)?;
    anthropic_compat::responses_request_to_anthropic(&intermediate, http_clients).await
}

async fn anthropic_request_to_gemini(
    body: &Bytes,
    http_clients: &ProxyHttpClients,
) -> Result<Bytes, String> {
    let intermediate = anthropic_compat::anthropic_request_to_responses(body, http_clients).await?;
    let visibility = responses_summary_visibility(&intermediate)?;
    let intermediate = responses_request_to_chat(&intermediate)?;
    let gemini =
        gemini_compat::chat_request_to_gemini_with_summary_visibility(&intermediate, visibility)?;
    preserve_anthropic_max_tokens_for_gemini(body, &gemini)
}

fn responses_summary_visibility(body: &Bytes) -> Result<SummaryVisibility, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    Ok(SummaryVisibility::from_responses_reasoning(
        value.get("reasoning"),
    ))
}

fn apply_summary_visibility_to_responses(
    body: &Bytes,
    visibility: SummaryVisibility,
) -> Result<Bytes, String> {
    let Some(summary) = visibility.responses_summary() else {
        return Ok(body.clone());
    };
    tracing::debug!(?visibility, "mapped summary visibility to Responses");
    update_json_body(body, |object| {
        let reasoning = object
            .entry("reasoning".to_string())
            .or_insert_with(|| json!({}));
        if !reasoning.is_object() {
            *reasoning = json!({});
        }
        if let Some(reasoning) = reasoning.as_object_mut() {
            reasoning.insert("summary".to_string(), summary);
        }
    })
}

fn preserve_anthropic_max_tokens_for_chat(
    anthropic_body: &Bytes,
    chat_body: &Bytes,
) -> Result<Bytes, String> {
    let Some(max_tokens) = anthropic_max_tokens(anthropic_body)? else {
        return Ok(chat_body.clone());
    };
    update_json_body(chat_body, |object| {
        object.insert(
            "max_completion_tokens".to_string(),
            Value::Number(max_tokens.into()),
        );
    })
}

fn preserve_anthropic_max_tokens_for_gemini(
    anthropic_body: &Bytes,
    gemini_body: &Bytes,
) -> Result<Bytes, String> {
    let Some(max_tokens) = anthropic_max_tokens(anthropic_body)? else {
        return Ok(gemini_body.clone());
    };
    update_json_body(gemini_body, |object| {
        let generation_config = object
            .entry("generationConfig".to_string())
            .or_insert_with(|| json!({}));
        if !generation_config.is_object() {
            *generation_config = json!({});
        }
        if let Some(config) = generation_config.as_object_mut() {
            config.insert(
                "maxOutputTokens".to_string(),
                Value::Number(max_tokens.into()),
            );
        }
    })
}

fn anthropic_max_tokens(body: &Bytes) -> Result<Option<i64>, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "Request body must be JSON.".to_string())?;
    Ok(value
        .as_object()
        .and_then(|object| object.get("max_tokens"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0))
}

fn update_json_body(
    body: &Bytes,
    update: impl FnOnce(&mut Map<String, Value>),
) -> Result<Bytes, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| "Converted request body must be JSON.".to_string())?;
    let Some(mut object) = value.as_object().cloned() else {
        return Err("Converted request body must be a JSON object.".to_string());
    };
    update(&mut object);
    serde_json::to_vec(&Value::Object(object))
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize request: {err}"))
}

fn gemini_response_to_anthropic(bytes: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let intermediate = gemini_compat::gemini_response_to_chat(bytes, model_hint)?;
    let intermediate = chat_response_to_responses(&intermediate)?;
    anthropic_compat::responses_response_to_anthropic(&intermediate, model_hint)
}

fn anthropic_response_to_gemini(bytes: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let intermediate = anthropic_compat::anthropic_response_to_responses(bytes)?;
    let intermediate = responses_response_to_chat(&intermediate, model_hint)?;
    gemini_compat::chat_response_to_gemini(&intermediate, model_hint)
}

fn chat_messages_to_responses_input(
    messages: &[Value],
) -> Result<(Vec<Value>, Option<String>), String> {
    let mut system_texts = Vec::new();
    let mut input = Vec::new();
    let mut has_user_message = false;

    for message in messages {
        let Some(message) = message.as_object() else {
            continue;
        };

        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        match role {
            "system" => push_chat_system_message(&mut system_texts, message),
            "user" => push_chat_user_message(&mut input, &mut has_user_message, message)?,
            "assistant" => push_chat_assistant_message(&mut input, &mut has_user_message, message)?,
            "tool" => push_chat_tool_message(&mut input, message),
            _ => {}
        }
    }

    let instructions = message::join_non_empty_lines(system_texts);
    Ok((input, instructions))
}

fn push_chat_system_message(system_texts: &mut Vec<String>, message: &Map<String, Value>) {
    if let Some(text) = message::extract_text_from_chat_content(message.get("content")) {
        system_texts.push(text);
    }
}

fn push_chat_user_message(
    input: &mut Vec<Value>,
    has_user_message: &mut bool,
    message: &Map<String, Value>,
) -> Result<(), String> {
    let parts =
        message::chat_content_to_responses_message_parts(message.get("content"), "input_text")?;
    if parts.is_empty() {
        return Ok(());
    }
    input.push(json!({ "type": "message", "role": "user", "content": parts }));
    *has_user_message = true;
    Ok(())
}

fn push_chat_assistant_message(
    input: &mut Vec<Value>,
    has_user_message: &mut bool,
    message: &Map<String, Value>,
) -> Result<(), String> {
    // Responses API expects assistant message content parts to use output types.
    // This matches OpenAI's schema and avoids errors like: "supported values are output_text/refusal".
    let parts =
        message::chat_content_to_responses_message_parts(message.get("content"), "output_text")?;
    let tool_calls = message::chat_tool_calls_to_responses_items(message.get("tool_calls"));
    let legacy_call = message::chat_function_call_to_responses_item(message.get("function_call"));

    let has_payload = !parts.is_empty() || !tool_calls.is_empty() || legacy_call.is_some();
    if has_payload && !*has_user_message {
        input.push(message::user_placeholder_item());
        *has_user_message = true;
    }

    if !parts.is_empty() {
        input.push(json!({ "type": "message", "role": "assistant", "content": parts }));
    }
    input.extend(tool_calls);
    if let Some(item) = legacy_call {
        input.push(item);
    }
    Ok(())
}

fn push_chat_tool_message(input: &mut Vec<Value>, message: &Map<String, Value>) {
    let call_id = message
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let output = message::chat_tool_content_to_responses_output(message.get("content"));
    input.push(json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output
    }));
}

fn map_chat_reasoning_effort_to_responses_reasoning(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::String(effort)) if !effort.trim().is_empty() => {
            Some(json!({ "effort": effort }))
        }
        Some(Value::Object(object)) if !object.is_empty() => Some(Value::Object(object.clone())),
        _ => None,
    }
}

fn map_responses_reasoning_to_chat_reasoning_effort(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::String(effort)) if !effort.trim().is_empty() => {
            Some(Value::String(effort.to_string()))
        }
        Some(Value::Object(object)) => object
            .get("effort")
            .and_then(Value::as_str)
            .filter(|effort| !effort.trim().is_empty())
            .map(|effort| Value::String(effort.to_string())),
        _ => None,
    }
}

fn append_responses_web_search_tool(
    output: &mut Map<String, Value>,
    web_search_options: Option<&Value>,
) {
    let tools = output
        .entry("tools".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !matches!(tools, Value::Array(_)) {
        *tools = Value::Array(Vec::new());
    }
    let Value::Array(items) = tools else {
        return;
    };

    let mut tool = Map::new();
    tool.insert("type".to_string(), Value::String("web_search".to_string()));
    if let Some(Value::Object(options)) = web_search_options {
        for (key, value) in options {
            tool.insert(key.clone(), value.clone());
        }
    }
    items.push(Value::Object(tool));
}

fn merge_chat_response_format_into_responses_text(
    output: &mut Map<String, Value>,
    response_format: &Value,
) {
    let Some(format) = map_chat_response_format_to_responses_text_format(response_format) else {
        return;
    };

    let text = output
        .entry("text".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !matches!(text, Value::Object(_)) {
        *text = Value::Object(Map::new());
    }
    let Value::Object(text_object) = text else {
        return;
    };
    text_object.insert("format".to_string(), format);
}

fn map_chat_response_format_to_responses_text_format(response_format: &Value) -> Option<Value> {
    let object = response_format.as_object()?;
    match object.get("type").and_then(Value::as_str) {
        Some("json_schema") => {
            let schema = object
                .get("json_schema")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            Some(json!({
                "type": "json_schema",
                "name": schema.get("name").cloned().unwrap_or_else(|| json!("response_schema")),
                "schema": schema.get("schema").cloned().unwrap_or_else(|| json!({})),
                "strict": schema.get("strict").cloned().unwrap_or_else(|| json!(false))
            }))
        }
        Some("json_object") => Some(json!({ "type": "json_object" })),
        Some("text") => Some(json!({ "type": "text" })),
        _ => None,
    }
}

fn map_responses_text_to_chat_response_format(text: Option<&Value>) -> Option<Value> {
    let format = text
        .and_then(Value::as_object)
        .and_then(|text| text.get("format"))?;
    let object = format.as_object()?;
    match object.get("type").and_then(Value::as_str) {
        Some("json_schema") => Some(json!({
            "type": "json_schema",
            "json_schema": {
                "name": object.get("name").cloned().unwrap_or_else(|| json!("response_schema")),
                "schema": object.get("schema").cloned().unwrap_or_else(|| json!({})),
                "strict": object.get("strict").cloned().unwrap_or_else(|| json!(false))
            }
        })),
        Some("json_object") => Some(json!({ "type": "json_object" })),
        Some("text") => None,
        _ => None,
    }
}

fn responses_response_to_chat(bytes: &Bytes, model_hint: Option<&str>) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| "Upstream response must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Upstream response must be a JSON object.".to_string());
    };

    let extracted = extract::extract_responses_output(&value);
    let content_parts = extracted.content_parts;
    let reasoning_text = extracted.reasoning_text;
    let tool_calls = extracted.tool_calls;
    let annotations = extracted.annotations;
    let audio = extracted.audio;
    let thinking_blocks = extracted.thinking_blocks;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-proxy");
    let created = object
        .get("created_at")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .filter(|_| model_hint.is_none())
        .or(model_hint)
        .unwrap_or("unknown");

    let usage = object
        .get("usage")
        .and_then(|usage| usage::map_usage_responses_to_chat(usage));

    let finish_reason =
        compat_reason::chat_finish_reason_from_response_object(object, !tool_calls.is_empty());

    let audio =
        audio.or_else(|| compat_content::chat_message_audio_from_responses_parts(&content_parts));
    let mut content = compat_content::chat_message_content_from_responses_parts(&content_parts);
    if audio.is_some() && chat_message_content_is_empty(&content) {
        content = Value::Null;
    }
    let mut message = json!({
        "role": "assistant",
        "content": content
    });
    if let Some(message) = message.as_object_mut() {
        if !reasoning_text.trim().is_empty() {
            message.insert(
                "reasoning_content".to_string(),
                Value::String(reasoning_text),
            );
        }
        if !thinking_blocks.is_empty() {
            message.insert("thinking_blocks".to_string(), Value::Array(thinking_blocks));
        }
        if !annotations.is_empty() {
            message.insert("annotations".to_string(), Value::Array(annotations));
        }
        if let Some(audio) = audio {
            message.insert("audio".to_string(), audio);
        }
    }
    if !tool_calls.is_empty() {
        if let Some(message) = message.as_object_mut() {
            message.insert("tool_calls".to_string(), Value::Array(tool_calls));
        }
    }

    let output = json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": message,
                "finish_reason": finish_reason
            }
        ],
        "usage": usage
    });

    serde_json::to_vec(&output)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

fn chat_response_to_responses(bytes: &Bytes) -> Result<Bytes, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| "Upstream response must be JSON.".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("Upstream response must be a JSON object.".to_string());
    };

    let first_message = object
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_object)
        .and_then(|choice| choice.get("message"))
        .and_then(Value::as_object);
    let response_parts = first_message
        .map(chat_response_message_to_responses_parts)
        .unwrap_or_default();
    let tool_calls = extract::extract_chat_tool_calls(&value);
    let parallel_tool_calls = tool_calls.len() > 1;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp-proxy");
    let created = object.get("created").and_then(Value::as_i64).unwrap_or(0);
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let finish_reason = object
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_object)
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str);
    let (status, incomplete_reason) =
        compat_reason::responses_status_from_chat_finish_reason(finish_reason);
    let status = status.unwrap_or("completed");
    let incomplete_details = incomplete_reason
        .map(|reason| json!({ "reason": reason }))
        .unwrap_or(Value::Null);

    let usage = object
        .get("usage")
        .and_then(|usage| usage::map_usage_chat_to_responses(usage));

    let mut output = Vec::new();
    if let Some(reasoning_item) =
        first_message.and_then(|message| chat_response_message_to_reasoning_item(message, status))
    {
        output.push(reasoning_item);
    }
    if !response_parts.is_empty() || tool_calls.is_empty() {
        output.push(json!({
            "type": "message",
            "id": "msg_proxy",
            "status": status,
            "role": "assistant",
            "content": response_parts
        }));
    }
    for call in tool_calls {
        let mut item = json!({
            "id": call.item_id,
            "type": "function_call",
            "status": "completed",
            "arguments": call.arguments,
            "call_id": call.call_id,
            "name": call.name
        });
        if let Some(fields) = call.provider_specific_fields {
            item["provider_specific_fields"] = fields;
        }
        output.push(item);
    }

    let output = json!({
        "id": id,
        "object": "response",
        "created_at": created,
        "status": status,
        "error": null,
        "incomplete_details": incomplete_details,
        "model": model,
        "parallel_tool_calls": parallel_tool_calls,
        "output": output,
        "usage": usage
    });

    serde_json::to_vec(&output)
        .map(Bytes::from)
        .map_err(|err| format!("Failed to serialize response: {err}"))
}

fn copy_key(source: &serde_json::Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key) {
        target.insert(key.to_string(), value.clone());
    }
}

fn copy_keys(
    source: &serde_json::Map<String, Value>,
    target: &mut Map<String, Value>,
    keys: &[&str],
) {
    for key in keys {
        copy_key(source, target, key);
    }
}

fn chat_message_content_is_empty(content: &Value) -> bool {
    match content {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(parts) => parts.is_empty(),
        _ => false,
    }
}

fn chat_response_message_to_reasoning_item(
    message: &Map<String, Value>,
    status: &str,
) -> Option<Value> {
    let mut summary_text = String::new();
    let mut encrypted_content = None;

    if let Some(thinking_blocks) = message.get("thinking_blocks").and_then(Value::as_array) {
        for block in thinking_blocks {
            let Some(block) = block.as_object() else {
                continue;
            };
            match block.get("type").and_then(Value::as_str) {
                Some("thinking") => {
                    if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                        summary_text.push_str(text);
                    }
                }
                Some("redacted_thinking") => {
                    if let Some(data) = block.get("data").and_then(Value::as_str) {
                        encrypted_content = Some(data.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    if summary_text.trim().is_empty() {
        if let Some(reasoning_content) = message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            summary_text = reasoning_content.to_string();
        }
    }

    if let Some(signature) = message
        .get("provider_specific_fields")
        .and_then(Value::as_object)
        .filter(|fields| fields.get("provider").and_then(Value::as_str) == Some("gemini"))
        .and_then(|fields| fields.get("thought_signatures"))
        .and_then(Value::as_array)
        .and_then(|signatures| signatures.iter().find_map(Value::as_str))
        .and_then(gemini_compat::encode_function_signature_carrier)
    {
        encrypted_content = Some(signature);
    }

    if summary_text.trim().is_empty() && encrypted_content.is_none() {
        return None;
    }

    let mut item = json!({
        "type": "reasoning",
        "id": "rs_proxy",
        "status": status,
        "summary": []
    });
    if let Some(item) = item.as_object_mut() {
        if !summary_text.trim().is_empty() {
            item.insert(
                "summary".to_string(),
                json!([{ "type": "summary_text", "text": summary_text }]),
            );
        }
        if let Some(encrypted_content) = encrypted_content {
            item.insert(
                "encrypted_content".to_string(),
                Value::String(encrypted_content),
            );
        }
    }
    Some(item)
}

fn chat_response_message_to_responses_parts(message: &Map<String, Value>) -> Vec<Value> {
    let annotations = message
        .get("annotations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let audio_part = chat_response_audio_part(message.get("audio"));

    match message.get("content") {
        Some(Value::String(text)) => {
            if text.is_empty() && annotations.is_empty() {
                audio_part.into_iter().collect()
            } else {
                let mut parts = vec![responses_output_text_part(text.clone(), annotations)];
                if let Some(audio_part) = audio_part {
                    parts.push(audio_part);
                }
                parts
            }
        }
        Some(Value::Array(parts)) => {
            let mut combined_text = String::new();
            let mut output_parts = Vec::new();

            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                match part.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" | "input_text" => {
                        if let Some(text) = message::extract_text_from_part(part) {
                            combined_text.push_str(&text);
                        }
                    }
                    "image_url" | "input_image" => {
                        let image_url = match part.get("image_url") {
                            Some(Value::String(url)) => Some(json!({ "url": url })),
                            Some(Value::Object(object)) => object
                                .get("url")
                                .and_then(Value::as_str)
                                .map(|url| json!({ "url": url })),
                            _ => None,
                        };
                        if let Some(image_url) = image_url {
                            output_parts.push(json!({
                                "type": "output_image",
                                "image_url": image_url
                            }));
                        }
                    }
                    _ => {}
                }
            }

            if !combined_text.is_empty() || !annotations.is_empty() {
                output_parts.insert(0, responses_output_text_part(combined_text, annotations));
            }
            if let Some(audio_part) = audio_part {
                output_parts.push(audio_part);
            }
            output_parts
        }
        Some(Value::Null) | None => audio_part.into_iter().collect(),
        Some(other) => {
            let mut parts = vec![responses_output_text_part(
                message::stringify_any_json(Some(other)),
                annotations,
            )];
            if let Some(audio_part) = audio_part {
                parts.push(audio_part);
            }
            parts
        }
    }
}

fn chat_response_audio_part(audio: Option<&Value>) -> Option<Value> {
    audio.map(|audio| {
        json!({
            "type": "output_audio",
            "audio": audio.clone()
        })
    })
}

fn responses_output_text_part(text: String, annotations: Vec<Value>) -> Value {
    json!({
        "type": "output_text",
        "text": text,
        "annotations": annotations
    })
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
mod tests;
