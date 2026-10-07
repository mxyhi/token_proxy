use crate::proxy::log::LogContext;

/// 捕获状态由日志任务接管；客户端取消时也能刷完已接收的有界缓冲。
pub(crate) struct ResponseBodyCapture(token_proxy_storage::body_capture::ResponseBodyCapture);

impl ResponseBodyCapture {
    pub(crate) fn new(context: &LogContext) -> Self {
        Self(token_proxy_storage::body_capture::ResponseBodyCapture::new(
            context.request_headers.is_some() || context.request_body.is_some(),
        ))
    }

    pub(crate) async fn push(&mut self, chunk: &[u8]) {
        self.0.push(chunk).await;
    }

    pub(crate) fn write_log(
        &mut self,
        log: std::sync::Arc<crate::proxy::log::LogWriter>,
        entry: crate::proxy::log::LogEntry,
    ) {
        log.write_detached_with_body(entry, self.0.take());
    }
}

/// 非 SSE HTTP 200 的诊断只保留有限前缀；正常 SSE 一旦识别就释放。
pub(crate) struct StreamDiagnostic {
    prefix: Vec<u8>,
    truncated: bool,
}

impl StreamDiagnostic {
    const LIMIT: usize = 64 * 1024;

    pub(crate) fn new() -> Self {
        Self {
            prefix: Vec::new(),
            truncated: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        let remaining = Self::LIMIT.saturating_sub(self.prefix.len());
        self.truncated |= chunk.len() > remaining;
        self.prefix
            .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }

    pub(crate) fn message(&self) -> Option<String> {
        if self.prefix.is_empty() {
            return None;
        }
        Some(format!(
            "Upstream returned a non-SSE response body{}: {}",
            if self.truncated {
                " (diagnostic truncated to 65536 bytes)"
            } else {
                ""
            },
            String::from_utf8_lossy(&self.prefix)
        ))
    }
}
