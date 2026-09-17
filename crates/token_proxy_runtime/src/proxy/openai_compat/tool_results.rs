//! 工具结果只与当前 assistant 声明配对；孤立内容不得丢失或伪装成调用。
use std::collections::HashSet;

use serde_json::Value;

pub(super) fn repair_orphan_results(messages: Vec<Value>) -> Vec<Value> {
    let mut output = Vec::with_capacity(messages.len());
    let mut pending = HashSet::new();
    let mut deferred = Vec::new();
    let mut orphan_count = 0;
    for mut message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role == "assistant" {
            output.append(&mut deferred);
            pending.clear();
            if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                pending.extend(calls.iter().filter_map(|call| {
                    call.get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned)
                }));
            }
            output.push(message);
            continue;
        }
        if role == "tool" {
            let id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !id.is_empty() && pending.remove(id) {
                output.push(message);
                if pending.is_empty() {
                    output.append(&mut deferred);
                }
                continue;
            }
            if let Some(object) = message.as_object_mut() {
                object.insert("role".to_string(), Value::String("user".to_string()));
                object.remove("tool_call_id");
            }
            orphan_count += 1;
        }
        // 普通内容等当前并行工具组结束后交付，避免插断 tool_calls → tool 序列。
        if pending.is_empty() {
            output.push(message);
        } else {
            deferred.push(message);
        }
    }
    output.append(&mut deferred);
    if orphan_count > 0 {
        tracing::debug!(
            orphan_count,
            "preserved orphan tool results as user content"
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parallel_results_remain_adjacent_and_orphans_keep_content() {
        let result = repair_orphan_results(vec![
            json!({"role":"assistant","tool_calls":[{"id":"a"},{"id":"b"}]}),
            json!({"role":"tool","tool_call_id":"a","content":"first"}),
            json!({"role":"tool","tool_call_id":"unknown","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}),
            json!({"role":"tool","tool_call_id":"b","content":"second"}),
            json!({"role":"tool","tool_call_id":"a","content":"duplicate"}),
            json!({"role":"tool","content":"missing id"}),
        ]);
        assert_eq!(result[2]["tool_call_id"], "b");
        assert_eq!(result[3]["role"], "user");
        assert_eq!(result[3]["content"][0]["type"], "image_url");
        assert_eq!(result[4], json!({"role":"user","content":"duplicate"}));
        assert_eq!(result[5], json!({"role":"user","content":"missing id"}));
    }
}
