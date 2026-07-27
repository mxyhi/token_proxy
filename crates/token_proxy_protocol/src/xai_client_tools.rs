use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

const TOOL_SEARCH_PROXY_NAME: &str = "tool_search";
const INTERNAL_X_SEARCH_TOOL_NAMES: [&str; 4] = [
    "x_user_search",
    "x_semantic_search",
    "x_keyword_search",
    "x_thread_fetch",
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct XaiClientToolMapping {
    custom_tools: HashSet<String>,
    // 仅用于 X Search 内部 trace 防误删；普通 function 本身不需要响应恢复。
    function_tools: HashSet<String>,
    namespace_tools: HashMap<String, NamespaceToolName>,
    tool_search: bool,
    filter_internal_x_search: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NamespaceToolName {
    namespace: String,
    name: String,
}

impl XaiClientToolMapping {
    pub fn is_empty(&self) -> bool {
        self.custom_tools.is_empty()
            && self.namespace_tools.is_empty()
            && !self.tool_search
            && !self.filter_internal_x_search
    }

    /// 启用 xAI 服务端 X Search 子调用过滤，避免客户端重复执行内部 trace。
    pub fn enable_internal_x_search_filter(&mut self) {
        self.filter_internal_x_search = true;
    }

    /// 返回当前响应映射是否需要过滤服务端 X Search trace。
    pub fn filters_internal_x_search(&self) -> bool {
        self.filter_internal_x_search
    }
}

/// 将 Codex 客户端专用工具降级为 xAI 可接受的 function，并返回可逆映射。
pub fn adapt_request(
    object: &mut Map<String, Value>,
) -> Result<(XaiClientToolMapping, bool), String> {
    let Some(tools) = object.get("tools").and_then(Value::as_array) else {
        return Ok((XaiClientToolMapping::default(), false));
    };
    if tools.is_empty() {
        return Ok((XaiClientToolMapping::default(), false));
    }

    let mut mapping = XaiClientToolMapping {
        namespace_tools: collect_namespace_tools(tools),
        ..Default::default()
    };
    let mut function_names = HashSet::new();
    let mut custom_names = HashSet::new();
    for tool in tools {
        let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or_default();
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty());
        match tool_type {
            "function" => {
                if let Some(name) = name {
                    function_names.insert(name.to_string());
                }
            }
            "custom" => {
                if let Some(name) = name {
                    custom_names.insert(name.to_string());
                }
            }
            "tool_search" => mapping.tool_search = true,
            _ => {}
        }
    }
    for name in &custom_names {
        if function_names.contains(name) {
            return Err(format!(
                "Custom tool {name} conflicts with a function tool of the same name."
            ));
        }
    }
    if mapping.tool_search
        && (function_names.contains(TOOL_SEARCH_PROXY_NAME)
            || custom_names.contains(TOOL_SEARCH_PROXY_NAME)
            || mapping.namespace_tools.contains_key(TOOL_SEARCH_PROXY_NAME))
    {
        return Err(format!(
            "Built-in tool_search conflicts with a declared tool named {TOOL_SEARCH_PROXY_NAME}."
        ));
    }
    mapping.function_tools = function_names;

    let flattened = crate::tool_identity::flatten_responses_namespaces(object, &[])? > 0;
    let mut lowered = Vec::new();
    let mut seen_search = false;
    let mut changed = flattened;
    let tools = object
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for tool in tools {
        let Some(mut tool_object) = tool.as_object().cloned() else {
            lowered.push(tool);
            continue;
        };
        match tool_object.get("type").and_then(Value::as_str) {
            Some("custom") => {
                let Some(name) = tool_object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                else {
                    lowered.push(Value::Object(tool_object));
                    continue;
                };
                mapping.custom_tools.insert(name);
                tool_object.insert("type".to_string(), json!("function"));
                tool_object.insert("parameters".to_string(), custom_tool_schema());
                tool_object.remove("format");
                lowered.push(Value::Object(tool_object));
                changed = true;
            }
            Some("tool_search") => {
                if !seen_search {
                    seen_search = true;
                    lowered.push(json!({
                        "type": "function",
                        "name": TOOL_SEARCH_PROXY_NAME,
                        "description": "Search and load Codex tools, plugins, connectors, and MCP namespaces for the current task.",
                        "parameters": {
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"],
                            "additionalProperties": true
                        }
                    }));
                }
                changed = true;
            }
            _ => lowered.push(Value::Object(tool_object)),
        }
    }
    if changed {
        object.insert("tools".to_string(), Value::Array(lowered));
    }
    if let Some(input) = object.get_mut("input") {
        changed |= rewrite_history(input, &mapping);
    }
    changed |= rewrite_tool_choice(object, &mapping);
    if changed {
        tracing::debug!(
            custom_tools = mapping.custom_tools.len(),
            namespace_tools = mapping.namespace_tools.len(),
            tool_search = mapping.tool_search,
            "adapted Codex client tools for xAI Responses"
        );
    }
    Ok((mapping, changed))
}

fn collect_namespace_tools(tools: &[Value]) -> HashMap<String, NamespaceToolName> {
    let mut names = HashMap::new();
    for tool in tools {
        if tool.get("type").and_then(Value::as_str) != Some("namespace") {
            continue;
        }
        let Some(namespace) = tool
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let children = tool
            .get("tools")
            .and_then(Value::as_array)
            .or_else(|| tool.get("children").and_then(Value::as_array));
        for child in children.into_iter().flatten() {
            if child.get("type").and_then(Value::as_str) != Some("function") {
                continue;
            }
            let Some(name) = child
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            names.insert(
                format!("{namespace}__{name}"),
                NamespaceToolName {
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                },
            );
        }
    }
    names
}

fn custom_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"input": {"type": "string"}},
        "required": ["input"],
        "additionalProperties": false
    })
}

fn rewrite_history(value: &mut Value, mapping: &XaiClientToolMapping) -> bool {
    match value {
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            rewrite_history(item, mapping) | changed
        }),
        Value::Object(object) => {
            let mut changed = match object.get("type").and_then(Value::as_str) {
                Some("custom_tool_call")
                    if object
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| mapping.custom_tools.contains(name)) =>
                {
                    object.insert("type".to_string(), json!("function_call"));
                    let input = object
                        .remove("input")
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_default();
                    object.insert(
                        "arguments".to_string(),
                        Value::String(json!({"input": input}).to_string()),
                    );
                    true
                }
                Some("custom_tool_call_output") => {
                    object.insert("type".to_string(), json!("function_call_output"));
                    stringify_field(object, "output");
                    true
                }
                Some("tool_search_call") if mapping.tool_search => {
                    object.insert("type".to_string(), json!("function_call"));
                    object.insert("name".to_string(), json!(TOOL_SEARCH_PROXY_NAME));
                    stringify_field(object, "arguments");
                    object.remove("execution");
                    true
                }
                Some("tool_search_output") if mapping.tool_search => {
                    object.insert("type".to_string(), json!("function_call_output"));
                    stringify_field(object, "output");
                    true
                }
                _ => false,
            };
            for child in object.values_mut() {
                changed |= rewrite_history(child, mapping);
            }
            changed
        }
        _ => false,
    }
}

fn stringify_field(object: &mut Map<String, Value>, key: &str) {
    let Some(value) = object.get_mut(key) else {
        return;
    };
    if value.is_string() {
        return;
    }
    *value = Value::String(if value.is_null() {
        String::new()
    } else {
        value.to_string()
    });
}

fn rewrite_tool_choice(object: &mut Map<String, Value>, mapping: &XaiClientToolMapping) -> bool {
    let Some(choice) = object.get_mut("tool_choice").and_then(Value::as_object_mut) else {
        return false;
    };
    let choice_type = choice.get("type").and_then(Value::as_str);
    let name = choice.get("name").and_then(Value::as_str);
    if choice_type == Some("custom") && name.is_some_and(|name| mapping.custom_tools.contains(name))
    {
        choice.insert("type".to_string(), json!("function"));
        return true;
    }
    if choice_type == Some("tool_search") && mapping.tool_search {
        *choice = json!({"type": "function", "name": TOOL_SEARCH_PROXY_NAME})
            .as_object()
            .cloned()
            .expect("tool choice literal is an object");
        return true;
    }
    false
}

pub fn restore_json_payload(
    bytes: &[u8],
    mapping: &XaiClientToolMapping,
) -> Result<(Vec<u8>, bool), String> {
    if mapping.is_empty() {
        return Ok((bytes.to_vec(), false));
    }
    let mut value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("Failed to parse xAI Responses client tool payload: {error}"))?;
    let changed = restore_value(&mut value, mapping);
    if !changed {
        return Ok((bytes.to_vec(), false));
    }
    serde_json::to_vec(&value)
        .map(|value| (value, true))
        .map_err(|error| format!("Failed to serialize xAI Responses client tool payload: {error}"))
}

/// 只过滤 xAI X Search 内部 trace，不恢复客户端专用工具类型。
pub fn filter_internal_x_search_json_payload(
    bytes: &[u8],
    mapping: &XaiClientToolMapping,
) -> Result<(Vec<u8>, bool), String> {
    if !mapping.filter_internal_x_search {
        return Ok((bytes.to_vec(), false));
    }
    let mut value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("Failed to parse xAI X Search response payload: {error}"))?;
    let changed = filter_internal_x_search_value(&mut value, mapping);
    if !changed {
        return Ok((bytes.to_vec(), false));
    }
    serde_json::to_vec(&value)
        .map(|value| (value, true))
        .map_err(|error| format!("Failed to serialize xAI X Search response payload: {error}"))
}

fn restore_value(value: &mut Value, mapping: &XaiClientToolMapping) -> bool {
    match value {
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            restore_value(item, mapping) | changed
        }),
        Value::Object(object) => {
            let mut changed = filter_internal_x_search_output(object, mapping);
            changed |= restore_call_object(object, mapping);
            for child in object.values_mut() {
                changed |= restore_value(child, mapping);
            }
            changed
        }
        _ => false,
    }
}

fn filter_internal_x_search_output(
    object: &mut Map<String, Value>,
    mapping: &XaiClientToolMapping,
) -> bool {
    let Some(output) = object.get_mut("output").and_then(Value::as_array_mut) else {
        return false;
    };
    let original_len = output.len();
    output.retain(|item| !is_internal_x_search_call(item, mapping));
    let dropped_count = original_len.saturating_sub(output.len());
    if dropped_count > 0 {
        tracing::debug!(
            dropped_count,
            "filtered internal xAI X Search response calls"
        );
    }
    dropped_count > 0
}

fn filter_internal_x_search_value(value: &mut Value, mapping: &XaiClientToolMapping) -> bool {
    match value {
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            filter_internal_x_search_value(item, mapping) | changed
        }),
        Value::Object(object) => {
            let mut changed = filter_internal_x_search_output(object, mapping);
            for child in object.values_mut() {
                changed |= filter_internal_x_search_value(child, mapping);
            }
            changed
        }
        _ => false,
    }
}

fn is_internal_x_search_call(item: &Value, mapping: &XaiClientToolMapping) -> bool {
    if !mapping.filter_internal_x_search {
        return false;
    }
    let Some(object) = item.as_object() else {
        return false;
    };
    let item_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !matches!(item_type, "function_call" | "custom_tool_call") {
        return false;
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !INTERNAL_X_SEARCH_TOOL_NAMES.contains(&name) {
        return false;
    }
    if object
        .get("namespace")
        .and_then(Value::as_str)
        .is_some_and(|namespace| !namespace.trim().is_empty())
    {
        return false;
    }
    if object
        .get("call_id")
        .and_then(Value::as_str)
        .is_some_and(|call_id| call_id.trim().starts_with("xs_call"))
    {
        return true;
    }
    item_type != "function_call"
        || !(mapping.function_tools.contains(name) || mapping.custom_tools.contains(name))
}

fn restore_call_object(object: &mut Map<String, Value>, mapping: &XaiClientToolMapping) -> bool {
    if object.get("type").and_then(Value::as_str) != Some("function_call") {
        return false;
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if mapping.custom_tools.contains(&name) {
        let input = custom_input(object.get("arguments"));
        object.insert("type".to_string(), json!("custom_tool_call"));
        object.insert("input".to_string(), Value::String(input));
        object.remove("arguments");
        object.remove("namespace");
        return true;
    }
    if mapping.tool_search && name == TOOL_SEARCH_PROXY_NAME {
        let arguments = parsed_arguments(object.get("arguments"));
        object.insert("type".to_string(), json!("tool_search_call"));
        object.insert("execution".to_string(), json!("client"));
        object.insert("arguments".to_string(), arguments);
        object.remove("name");
        object.remove("namespace");
        return true;
    }
    let Some(original) = mapping.namespace_tools.get(&name) else {
        return false;
    };
    object.insert("name".to_string(), Value::String(original.name.clone()));
    object.insert(
        "namespace".to_string(),
        Value::String(original.namespace.clone()),
    );
    true
}

fn custom_input(arguments: Option<&Value>) -> String {
    parsed_arguments(arguments)
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn parsed_arguments(arguments: Option<&Value>) -> Value {
    match arguments {
        Some(Value::String(arguments)) => {
            serde_json::from_str(arguments).unwrap_or_else(|_| json!({}))
        }
        Some(Value::Object(_)) => arguments.cloned().unwrap_or_else(|| json!({})),
        _ => json!({}),
    }
}

#[derive(Clone, Debug)]
struct StreamCall {
    kind: StreamCallKind,
    name: String,
    item_id: String,
    call_id: String,
    output_index: u64,
    arguments: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamCallKind {
    Custom,
    ToolSearch,
}

pub struct XaiClientToolStreamRestorer {
    mapping: XaiClientToolMapping,
    restore_client_tools: bool,
    calls: HashMap<String, StreamCall>,
    call_ids: HashMap<String, String>,
    output_indexes: HashMap<u64, String>,
    dropped_item_ids: HashSet<String>,
    dropped_call_ids: HashSet<String>,
    dropped_output_indexes: BTreeSet<u64>,
    next_sequence: Option<u64>,
}

impl XaiClientToolStreamRestorer {
    pub fn new(mapping: XaiClientToolMapping) -> Self {
        Self::with_restore_mode(mapping, true)
    }

    /// 仅过滤内部 trace，保留原始 function/custom 形态供后续格式转换。
    pub fn new_filter_only(mapping: XaiClientToolMapping) -> Self {
        Self::with_restore_mode(mapping, false)
    }

    fn with_restore_mode(mapping: XaiClientToolMapping, restore_client_tools: bool) -> Self {
        Self {
            mapping,
            restore_client_tools,
            calls: HashMap::new(),
            call_ids: HashMap::new(),
            output_indexes: HashMap::new(),
            dropped_item_ids: HashSet::new(),
            dropped_call_ids: HashSet::new(),
            dropped_output_indexes: BTreeSet::new(),
            next_sequence: None,
        }
    }

    /// 恢复一个 Responses SSE data JSON；单个上游事件可被抑制或扩展为多个客户端事件。
    pub fn restore_event(&mut self, data: &str) -> Result<Vec<String>, String> {
        let mut value: Value = serde_json::from_str(data)
            .map_err(|error| format!("Failed to parse xAI Responses stream event: {error}"))?;
        let sequence = value
            .get("sequence_number")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| self.next_sequence.unwrap_or_default());
        self.next_sequence.get_or_insert(sequence);
        if self.record_internal_x_search_call(&value) || self.references_dropped_call(&value) {
            return Ok(Vec::new());
        }
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if !self.restore_client_tools {
            filter_internal_x_search_value(&mut value, &self.mapping);
            return self.serialize_events(vec![value]);
        }

        let events = match event_type.as_str() {
            "response.output_item.added" => {
                self.record_call(&value);
                restore_value(&mut value, &self.mapping);
                vec![value]
            }
            "response.function_call_arguments.delta" => {
                if let Some(key) = self.call_key(&value) {
                    let delta = value
                        .get("delta")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if let Some(call) = self.calls.get_mut(&key) {
                        call.arguments.push_str(delta);
                    }
                    Vec::new()
                } else {
                    restore_namespace_event(&mut value, &self.mapping);
                    vec![value]
                }
            }
            "response.function_call_arguments.done" => {
                if let Some(key) = self.call_key(&value) {
                    self.finish_arguments(&key, &value)
                } else {
                    restore_namespace_event(&mut value, &self.mapping);
                    vec![value]
                }
            }
            "response.output_item.done" => {
                self.record_call(&value);
                if let Some(key) = self.call_key(&value) {
                    self.restore_done_item(&key, &mut value);
                    self.remove_call(&key);
                } else {
                    restore_value(&mut value, &self.mapping);
                }
                vec![value]
            }
            _ => {
                restore_value(&mut value, &self.mapping);
                restore_namespace_event(&mut value, &self.mapping);
                vec![value]
            }
        };

        self.serialize_events(events)
    }

    fn serialize_events(&mut self, events: Vec<Value>) -> Result<Vec<String>, String> {
        events
            .into_iter()
            .map(|mut event| {
                self.compact_output_index(&mut event);
                let sequence = self.next_sequence.unwrap_or_default();
                event["sequence_number"] = json!(sequence);
                self.next_sequence = Some(sequence.saturating_add(1));
                serde_json::to_string(&event).map_err(|error| {
                    format!("Failed to serialize xAI Responses stream event: {error}")
                })
            })
            .collect()
    }

    fn record_internal_x_search_call(&mut self, event: &Value) -> bool {
        let Some(item) = event.get("item") else {
            return false;
        };
        if !is_internal_x_search_call(item, &self.mapping) {
            return false;
        }
        if let Some(item_id) = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.dropped_item_ids.insert(item_id.to_string());
        }
        if let Some(call_id) = item
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.dropped_call_ids.insert(call_id.to_string());
        }
        if let Some(output_index) = event.get("output_index").and_then(Value::as_u64) {
            self.dropped_output_indexes.insert(output_index);
        }
        tracing::debug!(
            tool_name = item
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or_default(),
            "filtered internal xAI X Search stream call"
        );
        true
    }

    fn references_dropped_call(&self, event: &Value) -> bool {
        let item = event.get("item");
        let item_id = event
            .get("item_id")
            .or_else(|| item.and_then(|item| item.get("id")))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if item_id.is_some_and(|item_id| self.dropped_item_ids.contains(item_id)) {
            return true;
        }
        let call_id = event
            .get("call_id")
            .or_else(|| item.and_then(|item| item.get("call_id")))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if call_id.is_some_and(|call_id| self.dropped_call_ids.contains(call_id)) {
            return true;
        }
        event
            .get("output_index")
            .and_then(Value::as_u64)
            .is_some_and(|index| self.dropped_output_indexes.contains(&index))
    }

    fn compact_output_index(&self, event: &mut Value) {
        let Some(original) = event.get("output_index").and_then(Value::as_u64) else {
            return;
        };
        let removed_before = self.dropped_output_indexes.range(..original).count() as u64;
        event["output_index"] = json!(original.saturating_sub(removed_before));
    }

    fn record_call(&mut self, event: &Value) {
        let Some(item) = event.get("item").and_then(Value::as_object) else {
            return;
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let kind = if self.mapping.custom_tools.contains(&name) {
            StreamCallKind::Custom
        } else if self.mapping.tool_search && name == TOOL_SEARCH_PROXY_NAME {
            StreamCallKind::ToolSearch
        } else {
            return;
        };
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let key = if item_id.is_empty() {
            call_id.clone()
        } else {
            item_id.clone()
        };
        if key.is_empty() {
            return;
        }
        let output_index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let arguments = item
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.calls.entry(key.clone()).or_insert(StreamCall {
            kind,
            name,
            item_id,
            call_id: call_id.clone(),
            output_index,
            arguments,
        });
        if !call_id.is_empty() {
            self.call_ids.insert(call_id, key.clone());
        }
        self.output_indexes.insert(output_index, key);
    }

    fn call_key(&self, event: &Value) -> Option<String> {
        let item = event.get("item");
        let item_id = event
            .get("item_id")
            .or_else(|| item.and_then(|item| item.get("id")))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        if let Some(item_id) = item_id.filter(|item_id| self.calls.contains_key(*item_id)) {
            return Some(item_id.to_string());
        }
        let call_id = event
            .get("call_id")
            .or_else(|| item.and_then(|item| item.get("call_id")))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        if let Some(key) = call_id.and_then(|call_id| self.call_ids.get(call_id)) {
            return Some(key.clone());
        }
        event
            .get("output_index")
            .and_then(Value::as_u64)
            .and_then(|index| self.output_indexes.get(&index))
            .cloned()
    }

    fn finish_arguments(&mut self, key: &str, event: &Value) -> Vec<Value> {
        let Some(call) = self.calls.get_mut(key) else {
            return vec![event.clone()];
        };
        if let Some(arguments) = event.get("arguments").and_then(Value::as_str) {
            if !arguments.is_empty() {
                call.arguments = arguments.to_string();
            }
        }
        if call.kind == StreamCallKind::ToolSearch {
            return Vec::new();
        }
        let input = custom_input(Some(&Value::String(call.arguments.clone())));
        let mut events = Vec::new();
        if !input.is_empty() {
            events.push(json!({
                "type": "response.custom_tool_call_input.delta",
                "output_index": call.output_index,
                "item_id": call.item_id,
                "delta": input
            }));
        }
        events.push(json!({
            "type": "response.custom_tool_call_input.done",
            "output_index": call.output_index,
            "item_id": call.item_id,
            "call_id": call.call_id,
            "name": call.name,
            "input": input
        }));
        events
    }

    fn restore_done_item(&self, key: &str, event: &mut Value) {
        let Some(call) = self.calls.get(key) else {
            return;
        };
        let Some(item) = event.get_mut("item").and_then(Value::as_object_mut) else {
            return;
        };
        if let Some(arguments) = item
            .get("arguments")
            .and_then(Value::as_str)
            .filter(|arguments| !arguments.is_empty())
        {
            if call.kind == StreamCallKind::Custom {
                item.insert(
                    "input".to_string(),
                    Value::String(custom_input(Some(&Value::String(arguments.to_string())))),
                );
            } else {
                item.insert(
                    "arguments".to_string(),
                    parsed_arguments(Some(&json!(arguments))),
                );
            }
        } else if call.kind == StreamCallKind::Custom {
            item.insert(
                "input".to_string(),
                Value::String(custom_input(Some(&Value::String(call.arguments.clone())))),
            );
        } else {
            item.insert(
                "arguments".to_string(),
                parsed_arguments(Some(&Value::String(call.arguments.clone()))),
            );
        }
        item.insert(
            "type".to_string(),
            json!(match call.kind {
                StreamCallKind::Custom => "custom_tool_call",
                StreamCallKind::ToolSearch => "tool_search_call",
            }),
        );
        item.remove("namespace");
        if call.kind == StreamCallKind::Custom {
            item.remove("arguments");
        } else {
            item.remove("name");
            item.insert("execution".to_string(), json!("client"));
        }
    }

    fn remove_call(&mut self, key: &str) {
        let Some(call) = self.calls.remove(key) else {
            return;
        };
        self.call_ids.remove(&call.call_id);
        self.output_indexes.remove(&call.output_index);
    }
}

fn restore_namespace_event(event: &mut Value, mapping: &XaiClientToolMapping) -> bool {
    if event.get("type").and_then(Value::as_str) != Some("response.function_call_arguments.done") {
        return false;
    }
    let Some(name) = event
        .get("name")
        .and_then(Value::as_str)
        .and_then(|name| mapping.namespace_tools.get(name))
    else {
        return false;
    };
    event["name"] = Value::String(name.name.clone());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> XaiClientToolMapping {
        XaiClientToolMapping {
            custom_tools: HashSet::from(["exec".to_string()]),
            namespace_tools: HashMap::from([(
                "team__send".to_string(),
                NamespaceToolName {
                    namespace: "team".to_string(),
                    name: "send".to_string(),
                },
            )]),
            tool_search: true,
            ..Default::default()
        }
    }

    #[test]
    fn restores_json_client_and_namespace_calls() {
        let payload = br#"{"output":[{"type":"function_call","name":"exec","arguments":"{\"input\":\"pwd\"}"},{"type":"function_call","name":"tool_search","arguments":"{\"query\":\"git\"}"},{"type":"function_call","name":"team__send","arguments":"{}"}]}"#;
        let (restored, changed) = restore_json_payload(payload, &mapping()).expect("restore");
        let value: Value = serde_json::from_slice(&restored).expect("json");

        assert!(changed);
        assert_eq!(value["output"][0]["type"], "custom_tool_call");
        assert_eq!(value["output"][0]["input"], "pwd");
        assert_eq!(value["output"][1]["type"], "tool_search_call");
        assert_eq!(value["output"][1]["arguments"]["query"], "git");
        assert_eq!(value["output"][2]["name"], "send");
        assert_eq!(value["output"][2]["namespace"], "team");
    }

    #[test]
    fn filters_internal_x_search_calls_but_keeps_declared_same_name_function() {
        let mut request = json!({
            "tools": [
                {
                    "type": "function",
                    "name": "x_keyword_search",
                    "parameters": {"type": "object"}
                }
            ]
        });
        let request = request.as_object_mut().expect("request object");
        let (mut mapping, _) = adapt_request(request).expect("adapt request");
        mapping.enable_internal_x_search_filter();
        let payload = br#"{"output":[{"type":"custom_tool_call","name":"x_user_search","call_id":"xs_call_1"},{"type":"function_call","name":"x_semantic_search","call_id":"internal_without_prefix"},{"type":"function_call","name":"x_keyword_search","call_id":"client_call"},{"type":"message","content":[]}]}"#;

        let (restored, changed) = restore_json_payload(payload, &mapping).expect("restore");
        let value: Value = serde_json::from_slice(&restored).expect("json");
        let output = value["output"].as_array().expect("output");

        assert!(changed);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0]["name"], "x_keyword_search");
        assert_eq!(output[1]["type"], "message");
    }

    #[test]
    fn filter_only_keeps_client_tool_shape_for_later_format_conversion() {
        let mut mapping = mapping();
        mapping.enable_internal_x_search_filter();
        let payload = br#"{"output":[{"type":"custom_tool_call","name":"x_user_search","call_id":"xs_call_1"},{"type":"function_call","name":"exec","arguments":"{\"input\":\"pwd\"}"}]}"#;

        let (filtered, changed) =
            filter_internal_x_search_json_payload(payload, &mapping).expect("filter");
        let value: Value = serde_json::from_slice(&filtered).expect("json");

        assert!(changed);
        assert_eq!(value["output"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["output"][0]["type"], "function_call");
        assert_eq!(value["output"][0]["name"], "exec");
    }

    #[test]
    fn stream_restorer_buffers_custom_arguments_and_resequences() {
        let mut restorer = XaiClientToolStreamRestorer::new(mapping());
        let added = restorer
            .restore_event(r#"{"type":"response.output_item.added","sequence_number":7,"output_index":0,"item":{"type":"function_call","id":"i1","call_id":"c1","name":"exec","arguments":""},"upstream_extension":{"keep":true}}"#)
            .expect("added");
        assert_eq!(added.len(), 1);
        let added: Value = serde_json::from_str(&added[0]).expect("json");
        assert_eq!(added["sequence_number"], 7);
        assert_eq!(added["item"]["type"], "custom_tool_call");
        assert_eq!(added["upstream_extension"]["keep"], true);

        assert!(restorer
            .restore_event(r#"{"type":"response.function_call_arguments.delta","sequence_number":8,"output_index":0,"item_id":"i1","delta":"{\"input\":\"pw"}"#)
            .expect("delta")
            .is_empty());
        let done = restorer
            .restore_event(r#"{"type":"response.function_call_arguments.done","sequence_number":9,"output_index":0,"item_id":"i1","call_id":"c1","name":"exec","arguments":"{\"input\":\"pwd\"}"}"#)
            .expect("done");
        assert_eq!(done.len(), 2);
        let first: Value = serde_json::from_str(&done[0]).expect("json");
        let second: Value = serde_json::from_str(&done[1]).expect("json");
        assert_eq!(first["sequence_number"], 8);
        assert_eq!(first["delta"], "pwd");
        assert_eq!(second["sequence_number"], 9);
        assert_eq!(second["input"], "pwd");
    }

    #[test]
    fn stream_restorer_filters_internal_x_search_and_compacts_indexes() {
        let mut mapping = XaiClientToolMapping::default();
        mapping.enable_internal_x_search_filter();
        let mut restorer = XaiClientToolStreamRestorer::new(mapping);

        assert!(restorer
            .restore_event(r#"{"type":"response.output_item.added","sequence_number":7,"output_index":0,"item":{"type":"custom_tool_call","id":"internal_item","call_id":"xs_call_1","name":"x_user_search","input":"{}"}}"#)
            .expect("internal added")
            .is_empty());
        assert!(restorer
            .restore_event(r#"{"type":"response.function_call_arguments.delta","sequence_number":8,"output_index":0,"item_id":"internal_item","delta":"{}"}"#)
            .expect("internal delta")
            .is_empty());

        let visible = restorer
            .restore_event(r#"{"type":"response.output_item.added","sequence_number":9,"output_index":1,"item":{"type":"message","id":"message_1","content":[]}}"#)
            .expect("visible added");
        let visible: Value = serde_json::from_str(&visible[0]).expect("visible json");
        assert_eq!(visible["sequence_number"], 7);
        assert_eq!(visible["output_index"], 0);

        let completed = restorer
            .restore_event(r#"{"type":"response.completed","sequence_number":10,"response":{"output":[{"type":"custom_tool_call","id":"internal_item","call_id":"xs_call_1","name":"x_user_search"},{"type":"message","id":"message_1","content":[]}]}}"#)
            .expect("completed");
        let completed: Value = serde_json::from_str(&completed[0]).expect("completed json");
        assert_eq!(completed["sequence_number"], 8);
        assert_eq!(
            completed["response"]["output"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(completed["response"]["output"][0]["id"], "message_1");
    }

    #[test]
    fn stream_filter_only_keeps_client_tool_shape() {
        let mut mapping = mapping();
        mapping.enable_internal_x_search_filter();
        let mut restorer = XaiClientToolStreamRestorer::new_filter_only(mapping);

        assert!(restorer
            .restore_event(r#"{"type":"response.output_item.added","sequence_number":2,"output_index":0,"item":{"type":"custom_tool_call","id":"internal","call_id":"xs_call_1","name":"x_user_search"}}"#)
            .expect("internal")
            .is_empty());
        let visible = restorer
            .restore_event(r#"{"type":"response.output_item.added","sequence_number":3,"output_index":1,"item":{"type":"function_call","id":"client","call_id":"client_1","name":"exec","arguments":"{}"}}"#)
            .expect("visible");
        let visible: Value = serde_json::from_str(&visible[0]).expect("json");

        assert_eq!(visible["sequence_number"], 2);
        assert_eq!(visible["output_index"], 0);
        assert_eq!(visible["item"]["type"], "function_call");
    }
}
