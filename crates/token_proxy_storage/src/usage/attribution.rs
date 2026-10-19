//! 上游在 usage 里附带的 token 归因（`usage.attribution`）的落库形态。
//!
//! 逐条目明细 `items` 以会话条目 ID 为键；同一会话每次请求都会带上全部历史条目，
//! 单行可达数百 KB，且条目 ID 在 Request Detail 清空后无法再对应到内容。
//! 落库时按条目 ID 前缀（`msg_`、`rs_`、`ctco_` 等）汇总为 `items_by_type`，
//! 其余归因字段（`request_fields`、`server_instructions` 等）原样保留。

use serde_json::{Map, Value};
use std::collections::BTreeMap;

const TOKEN_FIELDS: [&str; 4] = [
    "input_tokens",
    "cached_tokens",
    "cache_write_tokens",
    "output_tokens",
];

#[derive(Default)]
struct TypeTotals {
    count: u64,
    tokens: [Option<u64>; 4],
}

/// 落库用的 usage JSON 文本；只有带逐条目归因时才复制一份再压缩。
pub(crate) fn stored_usage_json(usage: &Value) -> String {
    if usage.pointer("/attribution/items").is_none() {
        return usage.to_string();
    }
    let mut compacted = usage.clone();
    compact_attribution_items(&mut compacted);
    compacted.to_string()
}

/// 把 `attribution.items` 替换为按条目类型汇总的 `items_by_type`，返回是否发生改写。
///
/// 只认识 4 个 token 计数字段，以及由同样字段组成的 `content` 分段（分段之和等于条目合计，
/// 汇总只取条目合计）。遇到任何无法识别的结构都整段原样保留，宁可不压缩也不丢数据。
pub(crate) fn compact_attribution_items(usage: &mut Value) -> bool {
    let Some(attribution) = usage.get_mut("attribution").and_then(Value::as_object_mut) else {
        return false;
    };
    let Some(Value::Object(items)) = attribution.get("items") else {
        return false;
    };
    let Some(items_by_type) = summarize_items(items) else {
        return false;
    };
    attribution.remove("items");
    attribution.insert("items_by_type".to_string(), items_by_type);
    true
}

fn summarize_items(items: &Map<String, Value>) -> Option<Value> {
    let mut totals: BTreeMap<&str, TypeTotals> = BTreeMap::new();
    for (id, item) in items {
        let item = item.as_object()?;
        let known_fields = item
            .keys()
            .all(|key| key == "content" || TOKEN_FIELDS.contains(&key.as_str()));
        if !known_fields || !item.get("content").is_none_or(is_token_parts) {
            return None;
        }
        let entry = totals.entry(item_type(id)).or_default();
        entry.count += 1;
        for (slot, field) in entry.tokens.iter_mut().zip(TOKEN_FIELDS) {
            if let Some(value) = item.get(field) {
                *slot = Some(slot.unwrap_or(0).saturating_add(value.as_u64()?));
            }
        }
    }

    let items_by_type = totals
        .into_iter()
        .map(|(item_type, totals)| {
            let mut summary = Map::new();
            summary.insert("count".to_string(), totals.count.into());
            for (value, field) in totals.tokens.into_iter().zip(TOKEN_FIELDS) {
                if let Some(value) = value {
                    summary.insert(field.to_string(), value.into());
                }
            }
            (item_type.to_string(), Value::Object(summary))
        })
        .collect::<Map<_, _>>();
    Some(Value::Object(items_by_type))
}

fn is_token_parts(content: &Value) -> bool {
    content.as_array().is_some_and(|parts| {
        parts.iter().all(|part| {
            part.as_object().is_some_and(|part| {
                part.iter()
                    .all(|(key, value)| TOKEN_FIELDS.contains(&key.as_str()) && value.is_u64())
            })
        })
    })
}

/// 条目 ID 形如 `ctco_xxx`、`at_<uuid>`，前缀就是条目类型；没有前缀的归入 `other`。
fn item_type(id: &str) -> &str {
    id.find('_').map_or("other", |index| &id[..=index])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tokens(input: u64, cached: u64, output: u64) -> Value {
        json!({
            "input_tokens": input,
            "cached_tokens": cached,
            "cache_write_tokens": 0,
            "output_tokens": output
        })
    }

    fn usage_with_items() -> Value {
        let mut message_with_parts = tokens(30, 30, 0);
        message_with_parts["content"] = json!([tokens(10, 10, 0), tokens(20, 20, 0)]);
        json!({
            "input_tokens": 120,
            "input_tokens_details": {"cached_tokens": 100, "cache_write_tokens": 0},
            "output_tokens": 7,
            "total_tokens": 127,
            "attribution": {
                "request_fields": {"instructions": tokens(20, 20, 0)},
                "server_instructions": tokens(5, 0, 0),
                "items": {
                    "msg_a": message_with_parts,
                    "msg_b": tokens(15, 10, 2),
                    "ctco_c": tokens(50, 40, 0),
                    "at_d": tokens(0, 0, 5)
                }
            }
        })
    }

    #[test]
    fn compacts_items_into_type_totals_and_keeps_other_fields() {
        let mut usage = usage_with_items();
        assert!(compact_attribution_items(&mut usage));

        let mut expected = usage_with_items();
        let attribution = expected["attribution"].as_object_mut().unwrap();
        attribution.remove("items");
        attribution.insert(
            "items_by_type".to_string(),
            json!({
                "at_": {"count": 1, "input_tokens": 0, "cached_tokens": 0, "cache_write_tokens": 0, "output_tokens": 5},
                "ctco_": {"count": 1, "input_tokens": 50, "cached_tokens": 40, "cache_write_tokens": 0, "output_tokens": 0},
                "msg_": {"count": 2, "input_tokens": 45, "cached_tokens": 40, "cache_write_tokens": 0, "output_tokens": 2}
            }),
        );
        assert_eq!(usage, expected);
    }

    #[test]
    fn compaction_is_idempotent() {
        let mut usage = usage_with_items();
        assert!(compact_attribution_items(&mut usage));
        let compacted = usage.clone();

        assert!(!compact_attribution_items(&mut usage));
        assert_eq!(usage, compacted);
    }

    #[test]
    fn unrecognized_item_shapes_are_kept_verbatim() {
        for (id, item) in [
            ("msg_text", json!({"input_tokens": 1, "text": "hello"})),
            ("msg_float", json!({"input_tokens": 1.5})),
            (
                "msg_part",
                json!({"input_tokens": 1, "content": [{"input_tokens": 1, "type": "text"}]}),
            ),
            ("msg_scalar", json!(3)),
        ] {
            let mut usage = usage_with_items();
            usage["attribution"]["items"][id] = item;
            let original = usage.clone();

            assert!(!compact_attribution_items(&mut usage), "{id}");
            assert_eq!(usage, original, "{id}");
        }
    }

    #[test]
    fn stored_usage_json_keeps_usage_totals() {
        let plain = json!({"input_tokens": 3, "output_tokens": 1});
        assert_eq!(stored_usage_json(&plain), plain.to_string());

        let original = usage_with_items();
        let stored = stored_usage_json(&original);
        let stored_value: Value = serde_json::from_str(&stored).unwrap();
        assert!(stored_value.pointer("/attribution/items").is_none());

        let before = crate::usage::extract_usage_from_stored_json(&original.to_string()).unwrap();
        let after = crate::usage::extract_usage_from_stored_json(&stored).unwrap();
        assert_eq!(before.billable_usage, after.billable_usage);
        let totals = |snapshot: &crate::log::UsageSnapshot| {
            snapshot
                .usage
                .as_ref()
                .map(|usage| (usage.input_tokens, usage.output_tokens, usage.total_tokens))
        };
        assert_eq!(totals(&before), totals(&after));
        assert_eq!(totals(&after), Some((Some(120), Some(7), Some(127))));
    }
}
