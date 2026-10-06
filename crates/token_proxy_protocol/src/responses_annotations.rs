//! Responses 引用随文本块身份对账，转换为 Chat 的 URL citation 结构。
use serde_json::{json, Value};
use std::collections::HashSet;

use crate::responses_text::ResponsesTextRecovery;

#[derive(Default)]
pub struct ResponsesChatOutput {
    text: ResponsesTextRecovery,
    pending: Vec<PendingAnnotation>,
    emitted: HashSet<(usize, String)>,
    finished: bool,
}

struct PendingAnnotation {
    item_id: Option<String>,
    output_index: Option<u64>,
    content_index: u64,
    annotation: Value,
}

impl ResponsesChatOutput {
    pub fn process(&mut self, event: &Value) -> (Vec<String>, Vec<Value>) {
        if self.finished {
            return (Vec::new(), Vec::new());
        }
        let text = self.text.process(event);
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.annotation.added") => {
                if let Some(annotation) = event.get("annotation") {
                    self.remember(
                        event.get("item_id").and_then(Value::as_str),
                        event.get("output_index").and_then(Value::as_u64),
                        event
                            .get("content_index")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        annotation,
                    );
                }
            }
            Some("response.content_part.done") => {
                if let Some(part) = event.get("part") {
                    self.remember_part(
                        event.get("item_id").and_then(Value::as_str),
                        event.get("output_index").and_then(Value::as_u64),
                        event
                            .get("content_index")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        part,
                    );
                }
            }
            Some("response.output_item.done") => {
                if let Some(item) = event.get("item") {
                    self.remember_item(item, event.get("output_index").and_then(Value::as_u64));
                }
            }
            Some("response.completed" | "response.incomplete") => {
                if let Some(items) = event.pointer("/response/output").and_then(Value::as_array) {
                    for (index, item) in items.iter().enumerate() {
                        self.remember_item(item, Some(index as u64));
                    }
                }
                self.finished = true;
            }
            _ => {}
        }
        let mut annotations = Vec::new();
        // 引用可能早于正文到达；有可靠的块位置后才交付，不能猜测字节偏移。
        let mut pending = Vec::new();
        for entry in self.pending.drain(..) {
            let Some((identity, offset)) = self.text.part_position(
                entry.item_id.as_deref(),
                entry.output_index,
                entry.content_index,
            ) else {
                pending.push(entry);
                continue;
            };
            let Some(mut annotation) = chat_url_citation(&entry.annotation, offset) else {
                continue;
            };
            let ranges =
                if entry.annotation.get("type").and_then(Value::as_str) == Some("url_citation") {
                    let source = entry
                        .annotation
                        .get("url_citation")
                        .unwrap_or(&entry.annotation);
                    let (Some(start), Some(end)) = (
                        source
                            .get("start_index")
                            .and_then(Value::as_u64)
                            .and_then(|index| usize::try_from(index).ok()),
                        source
                            .get("end_index")
                            .and_then(Value::as_u64)
                            .and_then(|index| usize::try_from(index).ok()),
                    ) else {
                        continue;
                    };
                    let Some(ranges) = self.text.part_ranges(
                        entry.item_id.as_deref(),
                        entry.output_index,
                        entry.content_index,
                        start,
                        end,
                    ) else {
                        pending.push(entry);
                        continue;
                    };
                    Some(ranges)
                } else {
                    None
                };
            if !self
                .emitted
                .insert((identity, entry.annotation.to_string()))
            {
                continue;
            }
            if let Some(ranges) = ranges {
                for (start, end) in ranges {
                    annotation["url_citation"]["start_index"] = json!(start);
                    annotation["url_citation"]["end_index"] = json!(end);
                    annotations.push(annotation.clone());
                }
            } else {
                annotations.push(annotation);
            }
        }
        self.pending = pending;
        (text, annotations)
    }

    fn remember_item(&mut self, item: &Value, output_index: Option<u64>) {
        if item.get("type").and_then(Value::as_str) != Some("message")
            || item.get("role").and_then(Value::as_str) != Some("assistant")
        {
            return;
        }
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for (index, part) in content.iter().enumerate() {
                self.remember_part(
                    item.get("id").and_then(Value::as_str),
                    output_index,
                    index as u64,
                    part,
                );
            }
        }
    }

    fn remember_part(
        &mut self,
        item_id: Option<&str>,
        output_index: Option<u64>,
        content_index: u64,
        part: &Value,
    ) {
        if part.get("type").and_then(Value::as_str) != Some("output_text") {
            return;
        }
        if let Some(annotations) = part.get("annotations").and_then(Value::as_array) {
            for annotation in annotations {
                self.remember(item_id, output_index, content_index, annotation);
            }
        }
    }

    fn remember(
        &mut self,
        item_id: Option<&str>,
        output_index: Option<u64>,
        content_index: u64,
        annotation: &Value,
    ) {
        self.pending.push(PendingAnnotation {
            item_id: item_id.map(str::to_string),
            output_index,
            content_index,
            annotation: annotation.clone(),
        });
    }
}

pub fn response_chat_annotations(output: Option<&Value>) -> Vec<Value> {
    let mut annotations = Vec::new();
    let mut offset = 0;
    for item in output.and_then(Value::as_array).into_iter().flatten() {
        if item.get("type").and_then(Value::as_str) != Some("message")
            || item.get("role").and_then(Value::as_str) != Some("assistant")
        {
            continue;
        }
        for part in item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if part.get("type").and_then(Value::as_str) != Some("output_text") {
                continue;
            }
            let mut seen = HashSet::new();
            for annotation in part
                .get("annotations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if seen.insert(annotation.to_string()) {
                    if let Some(annotation) = chat_url_citation(annotation, offset) {
                        annotations.push(annotation);
                    }
                }
            }
            offset += part
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .count();
        }
    }
    annotations
}

fn chat_url_citation(annotation: &Value, offset: usize) -> Option<Value> {
    if annotation.get("type").and_then(Value::as_str) != Some("url_citation") {
        // 保留既有非 URL 引用扩展，不把它们改造成搜索引用。
        return Some(annotation.clone());
    }
    let citation = annotation.get("url_citation").unwrap_or(annotation);
    let url = citation.get("url")?.as_str()?.trim();
    if url.is_empty() {
        return None;
    }
    let start = citation.get("start_index")?.as_u64()?;
    let end = citation.get("end_index")?.as_u64()?;
    if end < start {
        return None;
    }
    Some(json!({
        "type":"url_citation",
        "url_citation": {
            "url":url,
            "title":citation.get("title").and_then(Value::as_str).unwrap_or(url),
            "start_index":start.checked_add(offset as u64)?,
            "end_index":end.checked_add(offset as u64)?
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_follow_interleaved_unicode_text_in_delivery_order() {
        let mut state = ResponsesChatOutput::default();
        let annotation = json!({"type":"url_citation","url":"https://example.com","title":"A","start_index":1,"end_index":2});
        state.process(&json!({"type":"response.output_text.delta","item_id":"a","output_index":0,"delta":"你"}));
        state.process(&json!({"type":"response.output_text.delta","item_id":"b","output_index":1,"delta":"🌍"}));
        // 引用尚未发送的尾部必须等待；不能以块起始位置预估未来字符坐标。
        assert!(state.process(&json!({"type":"response.output_text.annotation.added","item_id":"a","output_index":0,"annotation":annotation})).1.is_empty());
        let (_, citations) = state.process(&json!({"type":"response.output_text.delta","item_id":"a","output_index":0,"delta":"好"}));
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0]["url_citation"]["start_index"], 2);
        assert_eq!(citations[0]["url_citation"]["end_index"], 3);
        // 覆盖 A 全段的引用分成两个范围，不把 B 的字符算进 A 的来源。
        let (_, citations) = state.process(&json!({"type":"response.output_text.annotation.added","item_id":"a","output_index":0,"annotation":{"type":"url_citation","url":"https://example.com","title":"A","start_index":0,"end_index":2}}));
        assert_eq!(citations.len(), 2);
        assert_eq!(citations[0]["url_citation"]["start_index"], 0);
        assert_eq!(citations[0]["url_citation"]["end_index"], 1);
        assert_eq!(citations[1]["url_citation"]["start_index"], 2);
        assert_eq!(citations[1]["url_citation"]["end_index"], 3);
    }

    #[test]
    fn citations_wait_for_text_and_keep_same_url_on_distinct_parts() {
        let mut state = ResponsesChatOutput::default();
        let annotation = json!({"type":"url_citation","url":"https://example.com","title":"来源","start_index":0,"end_index":2});
        assert!(state.process(&json!({"type":"response.output_text.annotation.added","output_index":0,"content_index":0,"annotation":annotation})).1.is_empty());
        let first = state.process(&json!({"type":"response.output_text.delta","item_id":"m","output_index":0,"content_index":0,"delta":"你🌍"}));
        assert_eq!(first.1[0]["url_citation"]["start_index"], 0);
        let item = json!({"id":"m","type":"message","role":"assistant","content":[
            {"type":"output_text","text":"你🌍","annotations":[annotation.clone()]},
            {"type":"output_text","text":"文献","annotations":[annotation]}
        ]});
        let second = state
            .process(&json!({"type":"response.output_item.done","output_index":0,"item":item}));
        assert_eq!(second.0, vec!["文献"]);
        assert_eq!(second.1.len(), 1);
        assert_eq!(second.1[0]["url_citation"]["start_index"], 2);
        assert_eq!(second.1[0]["url_citation"]["end_index"], 4);
        let terminal = json!({"type":"response.incomplete","response":{"output":[item]}});
        assert!(state.process(&terminal).1.is_empty());
        assert!(state.process(&terminal).1.is_empty());
    }
}
