use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
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
}

impl ClientLifecycle {
    pub(crate) fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }

    pub(crate) fn note_log(&self) {
        self.logs.fetch_add(1, Ordering::Relaxed);
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
        self.lifecycle.canceled.store(true, Ordering::Release);
        tracing::debug!(path = %self.context.path, "client response lifecycle canceled");
    }

    fn finish_cancel(&self) {
        // 上游错误已即时写入；内层流释放后仅在没有任何日志时补请求级取消。
        if self.lifecycle.logs.load(Ordering::Relaxed) > 0 {
            return;
        }
        let mut context = self.context.clone();
        context.status = 499;
        self.log.clone().write_detached(build_log_entry(
            &context,
            UsageSnapshot::default(),
            Some(CLIENT_CANCELED_ERROR.to_string()),
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
