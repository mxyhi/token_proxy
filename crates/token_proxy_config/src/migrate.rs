use serde_json::{Map, Value};

use super::hot_model_mappings::default_hot_model_mappings;
use super::InboundApiFormat;

/// 将旧版 config 迁移为新版结构：
/// - 删除 `enable_api_format_conversion`
/// - `upstreams[].provider` -> `upstreams[].providers: string[]`
/// - 按旧开关补齐 `convert_from_map`
/// - 平铺 `api_keys` / `*_account_id` -> `credential` 判别联合
///
/// 事务语义：全部变换在 `Value` 克隆上完成，成功后才替换调用方传入的 `root`；
/// 任一上游冲突/错误时返回 `Err`，且传入 `Value` 字节语义不变。
/// 返回：是否发生了任何修改（用于决定是否写回配置文件）。
pub fn migrate_config_json(root: &mut Value) -> Result<bool, String> {
    if !needs_migration(root) {
        return Ok(false);
    }

    // clone-then-commit：失败时调用方 root 保持原样。
    let mut working = root.clone();
    let changed = apply_migrations(&mut working)?;
    *root = working;
    Ok(changed)
}

/// 只读检测是否需要迁移；不修改任何字段。
fn needs_migration(root: &Value) -> bool {
    let Some(root_obj) = root.as_object() else {
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
    let had_legacy_flat_credential = root_obj
        .get("upstreams")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.as_object().is_some_and(|obj| {
                    obj.contains_key("api_keys")
                        || obj.contains_key("kiro_account_id")
                        || obj.contains_key("codex_account_id")
                        || obj.contains_key("xai_account_id")
                        || !obj.contains_key("credential")
                })
            })
        });
    let had_legacy_upstream_strategy = root_obj
        .get("upstream_strategy")
        .and_then(Value::as_str)
        .is_some_and(|value| {
            matches!(value.trim(), "priority_fill_first" | "priority_round_robin")
        });
    let missing_hot_model_mappings = !root_obj.contains_key("hot_model_mappings");
    let has_legacy_model_discovery_refresh_secs =
        root_obj.contains_key("model_discovery_refresh_secs");

    had_legacy_enable
        || had_legacy_provider
        || had_legacy_api_key
        || had_legacy_flat_credential
        || had_legacy_upstream_strategy
        || missing_hot_model_mappings
        || has_legacy_model_discovery_refresh_secs
}

/// 在克隆上执行全部迁移步骤；成功返回是否有实质修改。
fn apply_migrations(root: &mut Value) -> Result<bool, String> {
    let Some(root_obj) = root.as_object_mut() else {
        return Ok(false);
    };

    let had_legacy_enable = root_obj.contains_key("enable_api_format_conversion");
    // 旧默认：true（README/前端默认值）
    let legacy_enable_conversion =
        take_bool(root_obj, "enable_api_format_conversion").unwrap_or(true);

    let mut changed = false;
    changed |= had_legacy_enable;
    changed |= migrate_legacy_upstream_strategy(root_obj);
    changed |= migrate_hot_model_mappings(root_obj);
    changed |= remove_legacy_model_discovery_refresh_secs(root_obj);

    let Some(upstreams_value) = root_obj.get_mut("upstreams") else {
        return Ok(changed);
    };
    let Some(upstreams) = upstreams_value.as_array_mut() else {
        return Ok(changed);
    };

    let upstream_count = upstreams.len();
    for upstream in upstreams.iter_mut() {
        changed |= migrate_single_upstream(upstream, legacy_enable_conversion)?;
    }

    if changed {
        tracing::info!(upstream_count, "config migration applied on working copy");
    }
    Ok(changed)
}

fn migrate_hot_model_mappings(root_obj: &mut Map<String, Value>) -> bool {
    if root_obj.contains_key("hot_model_mappings") {
        return false;
    }
    let value = serde_json::to_value(default_hot_model_mappings())
        .unwrap_or_else(|_| Value::Object(Map::new()));
    root_obj.insert("hot_model_mappings".to_string(), value);
    true
}

fn remove_legacy_model_discovery_refresh_secs(root_obj: &mut Map<String, Value>) -> bool {
    root_obj.remove("model_discovery_refresh_secs").is_some()
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

fn migrate_single_upstream(
    upstream: &mut Value,
    legacy_enable_conversion: bool,
) -> Result<bool, String> {
    let Some(obj) = upstream.as_object_mut() else {
        return Ok(false);
    };

    let mut changed = false;
    let upstream_id = obj
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>")
        .to_string();

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

    // api_key -> api_keys[]（随后统一折叠进 credential）
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

    changed |= migrate_flat_fields_to_credential(obj, &upstream_id)?;
    Ok(changed)
}

/// 将平铺 api_keys / kiro|codex|xai_account_id 折叠为 credential 联合；冲突硬失败。
/// 不记录 key / account_id 明文，仅打脱敏结构化日志。
fn migrate_flat_fields_to_credential(
    obj: &mut Map<String, Value>,
    upstream_id: &str,
) -> Result<bool, String> {
    let has_legacy_keys = obj.contains_key("api_keys")
        || obj.contains_key("kiro_account_id")
        || obj.contains_key("codex_account_id")
        || obj.contains_key("xai_account_id");
    let has_credential = obj.contains_key("credential");

    // 新 credential 与任一旧平铺字段并存：硬失败，禁止静默覆盖。
    if has_legacy_keys && has_credential {
        return Err(format!(
            "Upstream {upstream_id} cannot combine credential with legacy flat credential fields."
        ));
    }
    if !has_legacy_keys && has_credential {
        return Ok(false);
    }

    // 类型错误必须明确失败，禁止静默丢弃后落成 passthrough。
    let api_keys = take_string_array(obj, "api_keys", upstream_id)?;
    let kiro_account_id = take_optional_account_id(obj, "kiro_account_id", upstream_id)?;
    let codex_account_id = take_optional_account_id(obj, "codex_account_id", upstream_id)?;
    let xai_account_id = take_optional_account_id(obj, "xai_account_id", upstream_id)?;

    let account_bindings = [
        ("kiro", kiro_account_id.as_deref()),
        ("codex", codex_account_id.as_deref()),
        ("xai", xai_account_id.as_deref()),
    ]
    .into_iter()
    .filter_map(|(provider, account_id)| account_id.map(|id| (provider, id)))
    .collect::<Vec<_>>();

    if account_bindings.len() > 1 {
        return Err(format!(
            "Upstream {upstream_id} cannot pin multiple account ids during credential migration."
        ));
    }
    if !api_keys.is_empty() && !account_bindings.is_empty() {
        return Err(format!(
            "Upstream {upstream_id} cannot combine api_keys with account_id during credential migration."
        ));
    }

    let (credential, credential_type, account_provider) =
        if let Some((provider, account_id)) = account_bindings.first().copied() {
            let value = Value::Object(Map::from_iter([
                ("type".to_string(), Value::String("account".to_string())),
                ("provider".to_string(), Value::String(provider.to_string())),
                (
                    "account_id".to_string(),
                    Value::String(account_id.to_string()),
                ),
            ]));
            (value, "account", Some(provider))
        } else if !api_keys.is_empty() {
            let key_count = api_keys.len();
            let value = Value::Object(Map::from_iter([
                ("type".to_string(), Value::String("api_keys".to_string())),
                (
                    "api_keys".to_string(),
                    Value::Array(api_keys.into_iter().map(Value::String).collect::<Vec<_>>()),
                ),
            ]));
            // key 内容不入日志；仅记录数量。
            tracing::debug!(
                upstream_id,
                api_key_count = key_count,
                "folding flat api_keys into credential"
            );
            (value, "api_keys", None)
        } else {
            let value = Value::Object(Map::from_iter([(
                "type".to_string(),
                Value::String("passthrough".to_string()),
            )]));
            (value, "passthrough", None)
        };

    obj.insert("credential".to_string(), credential);
    // 脱敏：不记录 key / account_id。
    tracing::info!(
        upstream_id,
        credential_type,
        account_provider,
        "migrated upstream flat credential fields to credential union"
    );
    Ok(true)
}

/// 读取并移除字符串数组字段；缺失视为空；类型错误明确失败。
fn take_string_array(
    obj: &mut Map<String, Value>,
    key: &str,
    upstream_id: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = obj.remove(key) else {
        return Ok(Vec::new());
    };
    let Value::Array(items) = value else {
        return Err(format!(
            "Upstream {upstream_id} field {key} must be an array of strings during credential migration."
        ));
    };
    let mut output = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        let Value::String(text) = item else {
            return Err(format!(
                "Upstream {upstream_id} field {key} must contain only strings during credential migration."
            ));
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            output.push(trimmed.to_string());
        }
    }
    Ok(output)
}

/// 读取并移除可选 account id；string/null 合法，其它类型失败。
fn take_optional_account_id(
    obj: &mut Map<String, Value>,
    key: &str,
    upstream_id: &str,
) -> Result<Option<String>, String> {
    let Some(value) = obj.remove(key) else {
        return Ok(None);
    };
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Value::Null => Ok(None),
        _ => Err(format!(
            "Upstream {upstream_id} field {key} must be a string or null during credential migration."
        )),
    }
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
