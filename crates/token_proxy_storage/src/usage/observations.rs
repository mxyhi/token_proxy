//! 从协议产物记录工具/媒体事实，不保存正文、工具参数、搜索词或图片数据。
//! 观察值与供应商原始 usage 分开放置；它们不参与 token 定价。
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

use super::UsageSnapshot;

#[derive(Default)]
pub(super) struct UsageObservations {
    metadata: Map<String, Value>,
    items: BTreeMap<String, Map<String, Value>>,
    seen_calls: BTreeSet<String>,
    active_gemini_calls: BTreeMap<u64, (String, String)>,
    counters: Map<String, Value>,
    image_details: Map<String, Value>,
}

impl UsageObservations {
    pub(super) fn observe(&mut self, value: &Value) {
        let root = value.get("response").unwrap_or(value);
        for field in [
            "tool_usage",
            "prompt_cache_retention",
            "prompt_cache_options",
        ] {
            if let Some(value) = root.get(field).filter(|value| !value.is_null()) {
                super::merge_usage_snapshot(
                    self.metadata.entry(field).or_insert(Value::Null),
                    value,
                );
            }
        }
        if let Some(items) = root
            .get("output")
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
        {
            // 非空终态 output 是权威快照；为空时保留先前 output_item.done 的观察。
            self.items.retain(|key, _| !key.starts_with("responses:"));
            for (index, item) in items.iter().enumerate() {
                self.observe_response_item(item, index as u64);
            }
        }
        if value.get("type").and_then(Value::as_str) == Some("response.output_item.done") {
            if let Some(item) = value.get("item") {
                self.observe_response_item(
                    item,
                    value
                        .get("output_index")
                        .and_then(Value::as_u64)
                        .unwrap_or(self.items.len() as u64),
                );
            }
        }
        if let Some(blocks) = root.get("content").and_then(Value::as_array) {
            for (index, block) in blocks.iter().enumerate() {
                self.observe_claude_block(block, index as u64);
            }
        }
        if value.get("type").and_then(Value::as_str) == Some("content_block_start") {
            if let Some(block) = value.get("content_block") {
                self.observe_claude_block(
                    block,
                    value
                        .get("index")
                        .and_then(Value::as_u64)
                        .unwrap_or(self.items.len() as u64),
                );
            }
        }
        if let Some(choices) = root.get("choices").and_then(Value::as_array) {
            for (index, choice) in choices.iter().enumerate() {
                let choice_index = choice
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(index as u64);
                let message = choice.get("delta").or_else(|| choice.get("message"));
                if let Some(calls) = message
                    .and_then(|message| message.get("tool_calls"))
                    .and_then(Value::as_array)
                {
                    for (call_index, call) in calls.iter().enumerate() {
                        let call_index = call
                            .get("index")
                            .and_then(Value::as_u64)
                            .unwrap_or(call_index as u64);
                        self.items.insert(
                            format!("chat:{choice_index}:{call_index}"),
                            Map::from_iter([("tool_calls_requested".into(), json!(1))]),
                        );
                    }
                }
            }
        }
        self.observe_gemini(root);
        if matches!(
            value.get("type").and_then(Value::as_str),
            Some("image_generation.completed" | "image_edit.completed")
        ) {
            add_count(&mut self.counters, "image_count", 1);
            self.observe_image_details(value);
        }
        if let Some(images) = root.get("data").and_then(Value::as_array) {
            let count = images
                .iter()
                .filter(|image| image.get("b64_json").is_some() || image.get("url").is_some())
                .count();
            if count > 0 {
                self.counters.insert("image_count".into(), json!(count));
                self.observe_image_details(root);
                if let Some(first) = images.first() {
                    self.observe_image_details(first);
                }
            }
        }
        if let Some(video) = root.get("video").filter(|video| video.is_object()) {
            let mut details = Map::new();
            for field in ["duration", "resolution", "aspect_ratio"] {
                if let Some(value) = video.get(field).filter(|value| {
                    value.is_number() || value.as_str().is_some_and(|text| text.len() <= 256)
                }) {
                    details.insert(field.into(), value.clone());
                }
            }
            if !details.is_empty() {
                self.metadata.insert("video".into(), Value::Object(details));
            }
        }
    }

    fn observe_response_item(&mut self, item: &Value, index: u64) {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
        let id = item
            .get("id")
            .or_else(|| item.get("call_id"))
            .and_then(Value::as_str);
        let key = format!(
            "responses:{}",
            id.map(str::to_string).unwrap_or_else(|| index.to_string())
        );
        let counter = match kind {
            "web_search_call" => "web_search_calls",
            "file_search_call" => "file_search_calls",
            "code_interpreter_call" => "code_execution_calls",
            "mcp_call" => "mcp_calls",
            "image_generation_call" => "image_generation_calls",
            "custom_tool_call"
                if item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id.starts_with("xs_")) =>
            {
                "x_search_calls"
            }
            "custom_tool_call"
                if item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id.starts_with("as_")) =>
            {
                "document_search_calls"
            }
            "function_call" | "custom_tool_call" | "local_shell_call" | "shell_call"
            | "computer_call" | "apply_patch_call" => "tool_calls_requested",
            _ => return,
        };
        if matches!(
            item.get("status").and_then(Value::as_str),
            Some("failed" | "in_progress")
        ) {
            self.items.remove(&key);
            return;
        }
        let mut counts = Map::from_iter([(counter.into(), json!(1))]);
        if kind == "web_search_call" {
            if id.is_some_and(|id| id.starts_with("is_")) {
                counts.insert("image_search_calls".into(), json!(1));
            }
            if let Some(action) = item.get("action") {
                let queries = action
                    .get("queries")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .filter(|count| *count > 0)
                    .unwrap_or_else(|| {
                        usize::from(
                            action
                                .get("query")
                                .and_then(Value::as_str)
                                .is_some_and(|query| !query.trim().is_empty())
                                || action.get("type").and_then(Value::as_str) == Some("search"),
                        )
                    });
                add_count(&mut counts, "web_search_queries", queries as u64);
                if let Some(sources) = action.get("sources").and_then(Value::as_array) {
                    add_count(&mut counts, "num_sources_used", sources.len() as u64);
                }
            }
        }
        if kind == "image_generation_call" {
            counts.insert("image_count".into(), json!(1));
            self.observe_image_details(item);
        }
        self.items.insert(key, counts);
    }

    fn observe_claude_block(&mut self, block: &Value, index: u64) {
        let kind = block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut counts = Map::new();
        match kind {
            "tool_use" => {
                counts.insert("tool_calls_requested".into(), json!(1));
            }
            "server_tool_use"
                if block.get("name").and_then(Value::as_str) == Some("web_search") =>
            {
                counts.insert("web_search_queries".into(), json!(1));
            }
            "web_search_tool_result" => {
                let Some(results) = block.get("content").and_then(Value::as_array) else {
                    return;
                };
                counts.insert("web_search_calls".into(), json!(1));
                counts.insert(
                    "num_search_results".into(),
                    json!(results
                        .iter()
                        .filter(|result| result.get("type").and_then(Value::as_str)
                            == Some("web_search_result"))
                        .count()),
                );
            }
            _ => return,
        }
        let id = block
            .get("id")
            .or_else(|| block.get("tool_use_id"))
            .and_then(Value::as_str);
        self.items.insert(
            format!(
                "claude:{kind}:{}",
                id.map(str::to_string).unwrap_or_else(|| index.to_string())
            ),
            counts,
        );
    }

    fn observe_image_details(&mut self, value: &Value) {
        for field in [
            "size",
            "resolution",
            "image_size",
            "aspect_ratio",
            "quality",
            "background",
            "output_format",
            "mime_type",
            "input_fidelity",
        ] {
            if let Some(value) = value
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| value.len() <= 256)
            {
                self.image_details
                    .entry(field)
                    .or_insert_with(|| json!(value));
            }
        }
    }

    fn observe_gemini(&mut self, root: &Value) {
        let Some(candidates) = root.get("candidates").and_then(Value::as_array) else {
            return;
        };
        for (position, candidate) in candidates.iter().enumerate() {
            let index = candidate
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(position as u64);
            if let Some(grounding) = candidate
                .get("groundingMetadata")
                .or_else(|| candidate.get("grounding_metadata"))
            {
                let mut counts = Map::new();
                if let Some(queries) = grounding
                    .get("webSearchQueries")
                    .or_else(|| grounding.get("web_search_queries"))
                    .and_then(Value::as_array)
                {
                    add_count(
                        &mut counts,
                        "web_search_queries",
                        queries
                            .iter()
                            .filter(|q| q.as_str().is_some_and(|q| !q.trim().is_empty()))
                            .count() as u64,
                    );
                }
                if let Some(chunks) = grounding
                    .get("groundingChunks")
                    .or_else(|| grounding.get("grounding_chunks"))
                    .and_then(Value::as_array)
                {
                    for chunk in chunks {
                        for (field, count) in [
                            ("web", "num_sources_used"),
                            ("retrievedContext", "document_search_sources"),
                            ("maps", "maps_sources"),
                        ] {
                            if chunk.get(field).is_some() {
                                add_count(&mut counts, count, 1);
                            }
                        }
                    }
                }
                self.items
                    .insert(format!("gemini:grounding:{index}"), counts);
            }
            if let Some(urls) = candidate
                .pointer("/urlContextMetadata/urlMetadata")
                .and_then(Value::as_array)
            {
                self.items.insert(
                    format!("gemini:urls:{index}"),
                    Map::from_iter([("web_fetch_calls".into(), json!(urls.len()))]),
                );
            }
            let Some(parts) = candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
            else {
                continue;
            };
            for part in parts {
                if let Some(call) = part
                    .get("functionCall")
                    .or_else(|| part.get("function_call"))
                {
                    self.observe_gemini_call(index, call);
                }
                if part
                    .get("executableCode")
                    .or_else(|| part.get("executable_code"))
                    .is_some()
                {
                    add_count(&mut self.counters, "code_execution_calls", 1);
                }
                if let Some(inline) = part.get("inlineData").or_else(|| part.get("inline_data")) {
                    if let Some(mime) = inline
                        .get("mimeType")
                        .or_else(|| inline.get("mime_type"))
                        .and_then(Value::as_str)
                    {
                        if mime.starts_with("image/") {
                            add_count(&mut self.counters, "image_count", 1);
                            self.image_details
                                .entry("mime_type")
                                .or_insert_with(|| json!(mime));
                        }
                    }
                }
            }
        }
    }

    fn observe_gemini_call(&mut self, candidate: u64, call: &Value) {
        let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
        let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
        let partial = call.get("partialArgs").is_some() || call.get("willContinue").is_some();
        let continues = call
            .get("willContinue")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some((active_id, active_name)) = self.active_gemini_calls.get(&candidate) {
            if partial
                && (id.is_empty() || id == active_id)
                && (name.is_empty() || name == active_name)
            {
                if !continues {
                    self.active_gemini_calls.remove(&candidate);
                }
                return;
            }
        }
        self.active_gemini_calls.remove(&candidate);
        if (partial && name.is_empty())
            || (id.is_empty() && name.is_empty() && call.get("args").is_none())
        {
            return;
        }
        if continues {
            self.active_gemini_calls
                .insert(candidate, (id.to_string(), name.to_string()));
        }
        if !id.is_empty() && !self.seen_calls.insert(format!("{candidate}:{id}")) {
            return;
        }
        add_count(&mut self.counters, "tool_calls_requested", 1);
    }

    pub(super) fn attach(&self, snapshot: &mut UsageSnapshot) {
        let mut counts = self.counters.clone();
        for item in self.items.values() {
            for (field, value) in item {
                if let Some(value) = value.as_u64() {
                    // 例如完成搜索返回零条结果，这是显式观察值，不能当成缺失。
                    counts.entry(field.clone()).or_insert(json!(0));
                    add_count(&mut counts, field, value);
                }
            }
        }
        let mut observed = Map::new();
        if !counts.is_empty() {
            observed.insert("tools".into(), Value::Object(counts));
        }
        if !self.metadata.is_empty() {
            observed.insert(
                "response_metadata".into(),
                Value::Object(self.metadata.clone()),
            );
        }
        if !self.image_details.is_empty() {
            observed.insert(
                "image_details".into(),
                Value::Object(self.image_details.clone()),
            );
        }
        if !observed.is_empty() {
            let usage = snapshot.usage_json.get_or_insert_with(|| json!({}));
            if let Some(usage) = usage.as_object_mut() {
                usage.insert("_proxy_observed".into(), Value::Object(observed));
            }
        }
    }
}

fn add_count(counts: &mut Map<String, Value>, field: &str, value: u64) {
    if value > 0 {
        let total = counts
            .get(field)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(value);
        counts.insert(field.to_string(), json!(total));
    }
}
