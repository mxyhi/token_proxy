pub struct SseEventParser {
    buffer: Vec<u8>,
    current_data: String,
    max_line_bytes: usize,
    max_event_bytes: usize,
    failed: bool,
}

// 与 CPA 的大工具/图片事件预算保持同量级；按行与事件累计，不限制网络 chunk。
// 超过 50 MiB 的单行或合并 data 正文会显式终止流，不截断后继续成功。
pub const MAX_SSE_LINE_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_SSE_EVENT_BYTES: usize = 50 * 1024 * 1024;

const MAX_RESPONSES_JSON_DOCUMENTS: usize = 16;
const MAX_RESPONSES_JSON_BYTES: usize = 16 * 1024 * 1024;

impl Default for SseEventParser {
    fn default() -> Self {
        Self::new()
    }
}

pub fn split_responses_json_documents(payload: &[u8]) -> Option<Vec<Vec<u8>>> {
    let payload = payload.trim_ascii();
    if payload.is_empty() || payload.len() > MAX_RESPONSES_JSON_BYTES {
        return None;
    }
    let mut documents = Vec::with_capacity(2);
    let stream = serde_json::Deserializer::from_slice(payload).into_iter::<serde_json::Value>();
    for value in stream {
        let value = value.ok()?;
        let event_type = value
            .get("type")
            .and_then(serde_json::Value::as_str)?
            .trim();
        if event_type.is_empty() || event_type.contains(['\r', '\n']) {
            return None;
        }
        if documents.len() == MAX_RESPONSES_JSON_DOCUMENTS {
            return None;
        }
        documents.push(serde_json::to_vec(&value).ok()?);
    }
    (documents.len() > 1).then_some(documents)
}

impl SseEventParser {
    pub fn new() -> Self {
        Self::with_limits(MAX_SSE_LINE_BYTES, MAX_SSE_EVENT_BYTES)
    }

    fn with_limits(max_line_bytes: usize, max_event_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            current_data: String::new(),
            max_line_bytes,
            max_event_bytes,
            failed: false,
        }
    }

    pub fn push_chunk<F: FnMut(String)>(
        &mut self,
        chunk: &[u8],
        mut on_event: F,
    ) -> std::io::Result<()> {
        self.ensure_active()?;
        // 网络分片可能切在 UTF-8 字符中间；只在整行收齐后解码，并复用行缓冲。
        for part in chunk.split_inclusive(|byte| *byte == b'\n') {
            let terminated = part.last() == Some(&b'\n');
            let bytes = if terminated {
                &part[..part.len() - 1]
            } else {
                part
            };
            if bytes.len() > self.max_line_bytes.saturating_sub(self.buffer.len()) {
                return Err(self.fail_limit("line", self.max_line_bytes));
            }
            self.buffer.extend_from_slice(bytes);
            if terminated {
                self.process_buffered_line(&mut on_event)?;
            }
        }
        Ok(())
    }

    pub fn finish<F: FnMut(String)>(&mut self, mut on_event: F) -> std::io::Result<()> {
        self.ensure_active()?;
        if !self.buffer.is_empty() {
            self.process_buffered_line(&mut on_event)?;
        }
        self.flush_event(&mut on_event);
        Ok(())
    }

    fn ensure_active(&self) -> std::io::Result<()> {
        if self.failed {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SSE parser already failed after exceeding its size limit",
            ))
        } else {
            Ok(())
        }
    }

    fn fail_limit(&mut self, scope: &'static str, limit: usize) -> std::io::Error {
        self.failed = true;
        // 错误后立即释放大块缓存；禁止继续消费并把截断后的流误报成功。
        self.buffer = Vec::new();
        self.current_data = String::new();
        tracing::warn!(
            scope,
            limit_bytes = limit,
            "upstream SSE size limit exceeded"
        );
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Upstream SSE {scope} exceeds the {limit} byte limit"),
        )
    }

    fn process_buffered_line<F: FnMut(String)>(&mut self, on_event: &mut F) -> std::io::Result<()> {
        let mut buffer = std::mem::take(&mut self.buffer);
        if buffer.last() == Some(&b'\r') {
            buffer.pop();
        }
        let line = String::from_utf8_lossy(&buffer);
        if matches!(line, std::borrow::Cow::Owned(_)) {
            tracing::debug!(
                line_bytes = buffer.len(),
                "replaced invalid UTF-8 in SSE line"
            );
        }
        self.process_line(&line, on_event)?;
        buffer.clear();
        self.buffer = buffer;
        Ok(())
    }

    fn process_line<F: FnMut(String)>(
        &mut self,
        line: &str,
        on_event: &mut F,
    ) -> std::io::Result<()> {
        if line.is_empty() {
            self.flush_event(on_event);
            return Ok(());
        }
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim_start();
            let separator_bytes = usize::from(!self.current_data.is_empty());
            let additional = data.len().saturating_add(separator_bytes);
            if additional > self.max_event_bytes.saturating_sub(self.current_data.len()) {
                return Err(self.fail_limit("event", self.max_event_bytes));
            }
            if separator_bytes > 0 {
                self.current_data.push('\n');
            }
            self.current_data.push_str(data);
        }
        Ok(())
    }

    fn flush_event<F: FnMut(String)>(&mut self, on_event: &mut F) {
        if self.current_data.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.current_data);
        let data = data.trim();
        if data.is_empty() {
            return;
        }
        if let Some(documents) = split_responses_json_documents(data.as_bytes()) {
            tracing::debug!(
                document_count = documents.len(),
                payload_bytes = data.len(),
                "split concatenated Responses SSE JSON documents"
            );
            for document in documents {
                if let Ok(document) = String::from_utf8(document) {
                    on_event(document);
                }
            }
            return;
        }
        on_event(data.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{split_responses_json_documents, SseEventParser};

    #[test]
    fn utf8_survives_every_network_split_and_unterminated_final_line() {
        let data = "data: 你好🦀\r\n\r\ndata: 最后";
        for split in 0..=data.len() {
            let mut parser = SseEventParser::new();
            let mut events = Vec::new();
            parser
                .push_chunk(&data.as_bytes()[..split], |event| events.push(event))
                .unwrap();
            parser
                .push_chunk(&data.as_bytes()[split..], |event| events.push(event))
                .unwrap();
            parser.finish(|event| events.push(event)).unwrap();
            assert_eq!(events, ["你好🦀", "最后"], "split {split}");
        }
    }

    #[test]
    fn splits_concatenated_responses_json_documents() {
        let payload = br#"{"type":"response.in_progress"}{"type":"response.completed"}"#;

        let documents = split_responses_json_documents(payload).expect("split documents");

        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0], br#"{"type":"response.in_progress"}"#);
        assert_eq!(documents[1], br#"{"type":"response.completed"}"#);
    }

    #[test]
    fn parser_emits_each_concatenated_responses_document() {
        let mut parser = SseEventParser::new();
        let mut events = Vec::new();

        parser
            .push_chunk(
                b"data: {\"type\":\"response.in_progress\"}{\"type\":\"response.completed\"}\n\n",
                |event| events.push(event),
            )
            .unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0], r#"{"type":"response.in_progress"}"#);
        assert_eq!(events[1], r#"{"type":"response.completed"}"#);
    }

    #[test]
    fn does_not_split_valid_or_non_responses_payloads() {
        for payload in [
            br#"{"type":"response.completed"}"#.as_slice(),
            br#"{"value":1}{"value":2}"#.as_slice(),
            br#"{"type":"response.completed"} trailing"#.as_slice(),
        ] {
            assert!(split_responses_json_documents(payload).is_none());
        }
    }

    #[test]
    fn rejects_fragmented_oversized_line_and_releases_buffers() {
        let mut parser = SseEventParser::with_limits(16, 64);
        parser.push_chunk(b"data: 123456", |_| {}).unwrap();
        let error = parser
            .push_chunk(b"789012", |_| panic!("oversized event emitted"))
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("line"));
        assert_eq!(parser.buffer.capacity(), 0);
        assert_eq!(parser.current_data.capacity(), 0);
        assert!(parser.finish(|_| {}).is_err());
        assert!(parser.push_chunk(b"data: ok\n\n", |_| {}).is_err());
    }

    #[test]
    fn rejects_multiline_event_including_joining_newlines() {
        let mut parser = SseEventParser::with_limits(32, 8);
        parser.push_chunk(b"data: 1234\n", |_| {}).unwrap();
        let error = parser
            .push_chunk(b"data: 5678\n\n", |_| panic!("oversized event emitted"))
            .unwrap_err();
        assert!(error.to_string().contains("event"));
        assert_eq!(parser.current_data.capacity(), 0);
    }

    #[test]
    fn finish_checks_unterminated_final_line_event_budget() {
        let mut parser = SseEventParser::with_limits(32, 8);
        parser
            .push_chunk(b"data: 1234\ndata: 5678", |_| {})
            .unwrap();
        assert!(parser
            .finish(|_| panic!("oversized event emitted"))
            .is_err());
    }

    #[test]
    fn large_network_chunk_with_many_bounded_events_is_accepted() {
        let mut parser = SseEventParser::with_limits(16, 8);
        let chunk = "data: 12345678\n\n".repeat(10000);
        let mut count = 0;
        parser
            .push_chunk(chunk.as_bytes(), |data| {
                assert_eq!(data, "12345678");
                count += 1;
            })
            .unwrap();
        parser
            .finish(|_| panic!("unexpected trailing event"))
            .unwrap();
        assert_eq!(count, 10000);
    }

    #[test]
    fn exact_line_and_multiline_event_limits_survive_all_fragmentations() {
        let payload = b"data: 1234\ndata: 567\n\n";
        for split in 0..=payload.len() {
            let mut parser = SseEventParser::with_limits(10, 8);
            let mut events = Vec::new();
            parser
                .push_chunk(&payload[..split], |event| events.push(event))
                .unwrap();
            parser
                .push_chunk(&payload[split..], |event| events.push(event))
                .unwrap();
            parser.finish(|event| events.push(event)).unwrap();
            assert_eq!(events, ["1234\n567"]);
        }
    }
    #[test]
    fn default_budget_accepts_large_tool_argument_event() {
        let arguments = "x".repeat(1024 * 1024);
        let payload = format!(
            "data: {{\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{arguments}\"}}\n\n"
        );
        let mut parser = SseEventParser::new();
        let mut events = Vec::new();
        for chunk in payload.as_bytes().chunks(8191) {
            parser
                .push_chunk(chunk, |event| events.push(event))
                .unwrap();
        }
        parser.finish(|event| events.push(event)).unwrap();
        assert_eq!(events.len(), 1);
        let event: serde_json::Value = serde_json::from_str(&events[0]).unwrap();
        assert_eq!(event["delta"].as_str().unwrap().len(), arguments.len());
    }
}
