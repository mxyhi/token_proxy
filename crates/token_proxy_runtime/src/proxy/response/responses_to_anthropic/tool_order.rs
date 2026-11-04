//! Anthropic 的工具内容块必须连续；只缓存与当前工具交错的事件。
use std::collections::VecDeque;

use serde_json::{json, Value};

const MAX_PENDING_BYTES: usize = 1024 * 1024;
const MAX_PENDING_EVENTS: usize = 1024;

#[derive(Default)]
pub(super) struct ToolEventOrder {
    active: Option<String>,
    pending: VecDeque<(Value, usize)>,
    pending_bytes: usize,
}

impl ToolEventOrder {
    pub(super) fn push(&mut self, event: Value) -> Result<Vec<Value>, ()> {
        // 空文本 delta 不携带内容，不应占用交错缓冲；上游可能随工具参数大量附带。
        if event["type"].as_str() == Some("response.output_text.delta")
            && event["delta"].as_str() == Some("")
        {
            return Ok(Vec::new());
        }
        let terminal = matches!(
            event["type"].as_str(),
            Some("response.completed" | "response.incomplete")
        );
        if terminal {
            let mut ready = Vec::new();
            // 先用权威快照补齐仍打开的工具，再释放夹在参数之间的正文/其他工具。
            if let Some(items) = event.pointer("/response/output").and_then(Value::as_array) {
                for item in items.iter().filter(|item| item["type"] == "function_call") {
                    // 顺序快照沿普通路径及时排出，不能把所有终态工具误算作交错缓存。
                    ready.extend(
                        self.push(json!({"type":"response.output_item.done","item":item}))?,
                    );
                }
            }
            ready.extend(self.finish());
            ready.push(event);
            return Ok(ready);
        }
        if self.pending.is_empty()
            && self
                .active
                .as_deref()
                .is_none_or(|id| tool_id(&event) == Some(id))
        {
            self.track(&event);
            return Ok(vec![event]);
        }
        self.enqueue(event)?;
        Ok(self.drain(false))
    }

    pub(super) fn finish(&mut self) -> Vec<Value> {
        self.drain(true)
    }

    fn enqueue(&mut self, event: Value) -> Result<(), ()> {
        let bytes = event.to_string().len();
        if self.pending.len() >= MAX_PENDING_EVENTS
            || self.pending_bytes.saturating_add(bytes) > MAX_PENDING_BYTES
        {
            tracing::warn!(
                pending_events = self.pending.len(),
                pending_bytes = self.pending_bytes,
                "Anthropic interleaved tool buffer limit exceeded"
            );
            return Err(());
        }
        self.pending_bytes += bytes;
        self.pending.push_back((event, bytes));
        Ok(())
    }

    fn drain(&mut self, force: bool) -> Vec<Value> {
        let mut ready = Vec::new();
        while !self.pending.is_empty() {
            let position = match self.active.as_deref() {
                Some(id) => self
                    .pending
                    .iter()
                    .position(|(event, _)| tool_id(event) == Some(id)),
                None => Some(0),
            };
            let Some(position) = position else {
                if !force {
                    break;
                }
                // EOF 没有 done 时仍保留已收到的参数；转换器负责标记截断。
                self.active = None;
                continue;
            };
            let (event, bytes) = self.pending.remove(position).expect("pending event");
            self.pending_bytes -= bytes;
            self.track(&event);
            ready.push(event);
        }
        ready
    }

    fn track(&mut self, event: &Value) {
        if let Some(id) = tool_id(event) {
            self.active = if matches!(
                event["type"].as_str(),
                Some("response.function_call_arguments.done" | "response.output_item.done")
            ) {
                None
            } else {
                Some(id.to_string())
            };
        }
    }
}

fn tool_id(event: &Value) -> Option<&str> {
    match event["type"].as_str()? {
        "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
            event["item_id"].as_str()
        }
        "response.output_item.added" | "response.output_item.done"
            if event["item"]["type"] == "function_call" =>
        {
            event["item"]["id"].as_str()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_tool_snapshots_do_not_share_the_interleaving_budget() {
        let mut order = ToolEventOrder::default();
        let arguments = format!("{{\"text\":\"{}\"}}", "x".repeat(MAX_PENDING_BYTES / 2));
        let event = json!({"type":"response.completed","response":{"output":[
            {"type":"function_call","id":"a","arguments":arguments},
            {"type":"function_call","id":"b","arguments":arguments},
        ]}});
        let ready = order
            .push(event)
            .expect("sequential tools must not exceed the interleaving budget");
        assert_eq!(ready.len(), 3);
        assert_eq!(ready[2]["type"], "response.completed");
        assert!(order.pending.is_empty());
    }

    #[test]
    fn interleaved_buffer_is_bounded() {
        let mut order = ToolEventOrder::default();
        order.push(json!({"type":"response.output_item.added","item":{"type":"function_call","id":"a"}})).unwrap();
        assert!(order
            .push(
                json!({"type":"response.output_text.delta","delta":"x".repeat(MAX_PENDING_BYTES)})
            )
            .is_err());
        for _ in 0..MAX_PENDING_EVENTS {
            assert!(order
                .push(json!({"type":"response.output_text.delta","delta":"x"}))
                .unwrap()
                .is_empty());
        }
        assert!(order
            .push(json!({"type":"response.output_text.delta","delta":"x"}))
            .is_err());
    }

    #[test]
    fn empty_text_deltas_do_not_consume_the_interleaving_budget() {
        let mut order = ToolEventOrder::default();
        order.push(json!({"type":"response.output_item.added","item":{"type":"function_call","id":"a"}})).unwrap();
        // 上游可能随每个工具参数 delta 附带一条空文本 delta；它们不应占用交错缓冲。
        for _ in 0..(MAX_PENDING_EVENTS * 2) {
            assert!(order
                .push(json!({"type":"response.output_text.delta","delta":""}))
                .unwrap()
                .is_empty());
        }
        assert!(order.finish().is_empty());
    }
}
