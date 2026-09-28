use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
};

use axum::{
    body::{Body, Bytes, HttpBody},
    response::Response,
};
use http_body::{Frame, SizeHint};

use super::log::{build_log_entry, LogContext, LogWriter, UsageSnapshot};

pub(crate) const CLIENT_CANCELED_ERROR: &str = "client disconnected before completion";

#[derive(Default)]
pub(crate) struct ClientLifecycle {
    canceled: AtomicBool,
    logs: AtomicU64,
    deferred: Mutex<Option<DeferredError>>,
}

struct DeferredError {
    provider: String,
    upstream_id: String,
    account_id: Option<String>,
    status: u16,
    message: String,
}

impl ClientLifecycle {
    pub(crate) fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }

    pub(crate) fn note_log(&self, preserve_deferred: bool) {
        self.logs.fetch_add(1, Ordering::Relaxed);
        if !preserve_deferred {
            self.deferred
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
        }
    }

    pub(crate) fn remember_transport_error(
        &self,
        provider: &str,
        upstream_id: &str,
        account_id: Option<&str>,
        status: u16,
        message: &str,
    ) {
        *self.deferred.lock().unwrap_or_else(|e| e.into_inner()) = Some(DeferredError {
            provider: provider.to_string(),
            upstream_id: upstream_id.to_string(),
            account_id: account_id.map(str::to_string),
            status,
            message: message.to_string(),
        });
    }
}

pub(crate) struct ClientGuard {
    lifecycle: Arc<ClientLifecycle>,
    context: LogContext,
    log: Arc<LogWriter>,
}

impl ClientGuard {
    pub(crate) fn new(
        lifecycle: Arc<ClientLifecycle>,
        context: LogContext,
        log: Arc<LogWriter>,
    ) -> Self {
        Self {
            lifecycle,
            context,
            log,
        }
    }

    fn cancel(&self) {
        // 先为历史错误保留完成序号，再释放后备流。否则延后落库的无 usage
        // 诊断会获得更大序号，错误取代后备流成为该请求的唯一账单记录。
        if self
            .lifecycle
            .deferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            self.context.timings.reserve_billing_attempt();
        }
        self.lifecycle.canceled.store(true, Ordering::Release);
        tracing::debug!(path = %self.context.path, "client response lifecycle canceled");
    }

    fn finish_cancel(&self) {
        // 内层流先释放并写已收集的 usage。没有流日志时才补请求级诊断，
        // 已发生的 transport 错误优先于取消，不更改账号冷却或触发重试。
        let deferred = self
            .lifecycle
            .deferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let mut context = self.context.clone();
        let message = if let Some(error) = deferred {
            context.provider = error.provider;
            context.upstream_id = error.upstream_id;
            context.account_id = error.account_id;
            context.status = error.status;
            error.message
        } else if self.lifecycle.logs.load(Ordering::Relaxed) == 0 {
            context.status = 499;
            CLIENT_CANCELED_ERROR.to_string()
        } else {
            return;
        };
        self.log.clone().write_detached(build_log_entry(
            &context,
            UsageSnapshot::default(),
            Some(message),
        ));
    }
}

// 只包装最外层 handler，永远不包装单个 race/hedge 候选。
// 显式 Drop 顺序保证内部流在释放前已看到客户端取消标记。
pub(crate) struct ClientRequest<F> {
    inner: Option<Pin<Box<F>>>,
    guard: Option<ClientGuard>,
}

impl<F> ClientRequest<F> {
    pub(crate) fn new(inner: F, guard: ClientGuard) -> Self {
        Self {
            inner: Some(Box::pin(inner)),
            guard: Some(guard),
        }
    }
}

impl<F: Future<Output = Response>> Future for ClientRequest<F> {
    type Output = Response;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Response> {
        let response = std::task::ready!(self
            .inner
            .as_mut()
            .expect("request future")
            .as_mut()
            .poll(cx));
        let (parts, body) = response.into_parts();
        let guard = self.guard.take();
        self.inner.take();
        let body = if body.is_end_stream() {
            body
        } else {
            Body::new(ClientBody {
                inner: Some(body),
                guard,
            })
        };
        Poll::Ready(Response::from_parts(parts, body))
    }
}

impl<F> Drop for ClientRequest<F> {
    fn drop(&mut self) {
        if let Some(guard) = self.guard.take() {
            guard.cancel();
            drop(self.inner.take());
            guard.finish_cancel();
        }
    }
}

struct ClientBody {
    inner: Option<Body>,
    guard: Option<ClientGuard>,
}

impl HttpBody for ClientBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        let inner = self.inner.as_mut().expect("response body");
        let frame = Pin::new(&mut *inner).poll_frame(cx);
        // EOF、真实 body 错误和最后一个 frame 都是正常终结；保留 trailers 与 size hint。
        if matches!(frame, Poll::Ready(None | Some(Err(_)))) || inner.is_end_stream() {
            self.guard.take();
        }
        frame
    }

    fn is_end_stream(&self) -> bool {
        self.inner.as_ref().is_none_or(HttpBody::is_end_stream)
    }

    fn size_hint(&self) -> SizeHint {
        self.inner
            .as_ref()
            .map(HttpBody::size_hint)
            .unwrap_or_default()
    }
}

impl Drop for ClientBody {
    fn drop(&mut self) {
        if let Some(guard) = self.guard.take() {
            guard.cancel();
            drop(self.inner.take());
            guard.finish_cancel();
        }
    }
}
