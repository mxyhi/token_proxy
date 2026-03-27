use serde_json::{Map, Value};

use super::InboundApiFormat;

/// 将旧版 config（含 `enable_api_format_conversion` / `upstreams[].provider`）迁移为新版结构：
/// - 删除 `enable_api_format_conversion`
/// - `upstreams[].provider` -> `upstreams[].providers: string[]`
/// - 按旧开关补齐 `convert_from_map`（用于保持旧行为，且最终会写回删除旧字段）
///
/// 返回：是否发生了任何修改（用于决定是否写回配置文件）。
pub(super) fn migrate_config_json(root: &mut Value) -> bool {
    let Some(root_obj) = root.as_object_mut() else {
        return false;
    };

    let had_legacy_enable = root_obj.contains_key("enable_api_format_conversion");
    let had_legacy_provider = root_obj
        .get("upstreams")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.as_object()
                    .is_some_and(|obj| obj.contains_key("provider"))
            })
        });
    let had_legacy_api_key = root_obj
        .get("upstreams")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.as_object()
                    .is_some_and(|obj| obj.contains_key("api_key"))
            })
        });
    let had_legacy_upstream_strategy = root_obj
        .get("upstream_strategy")
        .and_then(Value::as_str)
        .is_some_and(|value| {
            matches!(value.trim(), "priority_fill_first" | "priority_round_robin")
        });
    let is_legacy_config = had_legacy_enable
        || had_legacy_provider
        || had_legacy_api_key
        || had_legacy_upstream_strategy;

    // 仅当检测到旧字段时才进行迁移；否则避免对新配置做“默认填充”，改变用户语义。
    if !is_legacy_config {
        return false;
    }

    let legacy_enable_conversion = take_bool(root_obj, "enable_api_format_conversion")
        // 旧默认：true（README/前端默认值）
        .unwrap_or(true);

    let mut changed = false;
    changed |= had_legacy_enable;
    changed |= migrate_legacy_upstream_strategy(root_obj);

    let Some(upstreams_value) = root_obj.get_mut("upstreams") else {
        return changed;
    };
    let Some(upstreams) = upstreams_value.as_array_mut() else {
        return changed;
    };

    for upstream in upstreams {
        changed |= migrate_single_upstream(upstream, legacy_enable_conversion);
    }

    changed
}

fn migrate_legacy_upstream_strategy(root_obj: &mut Map<String, Value>) -> bool {
    let Some(value) = root_obj.get("upstream_strategy").and_then(Value::as_str) else {
        return false;
    };
    let order = match value.trim() {
        "priority_fill_first" => "fill_first",
        "priority_round_robin" => "round_robin",
        _ => return false,
    };

    root_obj.insert(
        "upstream_strategy".to_string(),
        Value::Object(Map::from_iter([
            ("order".to_string(), Value::String(order.to_string())),
            (
                "dispatch".to_string(),
                Value::Object(Map::from_iter([(
                    "type".to_string(),
                    Value::String("serial".to_string()),
                )])),
            ),
        ])),
    );
    true
}

fn migrate_single_upstream(upstream: &mut Value, legacy_enable_conversion: bool) -> bool {
    let Some(obj) = upstream.as_object_mut() else {
        return false;
    };

    let mut changed = false;

    // provider -> providers[]
    if let Some(provider_value) = obj.remove("provider") {
        changed = true;
        if let Some(provider) = provider_value
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            match obj.get_mut("providers") {
                Some(Value::Array(items)) => {
                    // 若已经是新版（用户手动加了 providers），则把旧 provider 合并进去。
                    if !items.iter().any(|v| v.as_str() == Some(provider)) {
                        items.push(Value::String(provider.to_string()));
                    }
                }
                _ => {
                    obj.insert(
                        "providers".to_string(),
                        Value::Array(vec![Value::String(provider.to_string())]),
                    );
                }
            }
        }
    }

    // api_key -> api_keys[]
    if let Some(api_key_value) = obj.remove("api_key") {
        changed = true;
        if let Some(api_key) = api_key_value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            merge_api_key_into_api_keys(obj, api_key);
        }
    }

    // 若用户已经写了 providers，但写成非数组，保持原样，让后续类型反序列化报错给出明确提示。

    // 旧版全局开关迁移：以 `convert_from_map` 显式表达允许转换的入站格式。
    // 方案 A 语义：convert_from_map 为空 => 仅 native；非空则允许对应入站格式转换后使用该 provider。
    if legacy_enable_conversion {
        // 旧默认 true：尽量保持原有“全局允许跨格式 fallback/转换”的体验。
        // 迁移策略：若 convert_from_map 缺失，则为当前 upstream 的每个 provider 注入“允许所有入站格式”。
        if !obj.contains_key("convert_from_map") {
            if let Some(providers) = read_providers(obj) {
                let mut map = Map::new();
                for provider in providers {
                    map.insert(provider, all_inbound_formats_value());
                }
                obj.insert("convert_from_map".to_string(), Value::Object(map));
                changed = true;
            }
        }
    }

    changed
}

fn read_providers(obj: &Map<String, Value>) -> Option<Vec<String>> {
    let Value::Array(items) = obj.get("providers")? else {
        return None;
    };
    let mut output = Vec::with_capacity(items.len());
    for item in items {
        let Some(value) = item.as_str().map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        output.push(value.to_string());
    }
    Some(output)
}

fn merge_api_key_into_api_keys(obj: &mut Map<String, Value>, api_key: &str) {
    match obj.get_mut("api_keys") {
        Some(Value::Array(items)) => {
            if !items.iter().any(|value| value.as_str() == Some(api_key)) {
                items.push(Value::String(api_key.to_string()));
            }
        }
        _ => {
            obj.insert(
                "api_keys".to_string(),
                Value::Array(vec![Value::String(api_key.to_string())]),
            );
        }
    }
}

fn all_inbound_formats_value() -> Value {
    Value::Array(vec![
        Value::String(inbound_format_name(InboundApiFormat::OpenaiChat).to_string()),
        Value::String(inbound_format_name(InboundApiFormat::OpenaiResponses).to_string()),
        Value::String(inbound_format_name(InboundApiFormat::AnthropicMessages).to_string()),
        Value::String(inbound_format_name(InboundApiFormat::Gemini).to_string()),
    ])
}

fn inbound_format_name(format: InboundApiFormat) -> &'static str {
    match format {
        InboundApiFormat::OpenaiChat => "openai_chat",
        InboundApiFormat::OpenaiResponses => "openai_responses",
        InboundApiFormat::AnthropicMessages => "anthropic_messages",
        InboundApiFormat::Gemini => "gemini",
    }
}

fn take_bool(obj: &mut Map<String, Value>, key: &str) -> Option<bool> {
    obj.remove(key).and_then(|value| value.as_bool())
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "migrate.test.rs"]
mod tests;
