use axum::body::{Body, Bytes};
use futures_util::StreamExt;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const IN_MEMORY_LIMIT_BYTES: usize = 512 * 1024;
const TEMP_FILE_PREFIX: &str = "token_proxy_body";
const FILE_READ_CHUNK_BYTES: usize = 64 * 1024;

static TEMP_FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

// 将入站请求体缓存为“可重放”形式：小体积保留在内存，超过阈值则落盘到临时文件。
// 这样可以在上游重试/降级时，重复发送同一份请求体（代价是需要先完整读取请求体）。
pub(crate) struct ReplayableBody {
    inner: ReplayableBodyInner,
    len: u64,
}

enum ReplayableBodyInner {
    InMemory(Bytes),
    TempFile { path: PathBuf },
}

impl ReplayableBody {
    pub(crate) fn from_bytes(bytes: Bytes) -> Self {
        Self {
            len: bytes.len() as u64,
            inner: ReplayableBodyInner::InMemory(bytes),
        }
    }

    pub(crate) async fn from_body(body: Body) -> Result<Self, std::io::Error> {
        let mut stream = body.into_data_stream();
        let mut len = 0_u64;
        let mut buffer: Vec<u8> = Vec::new();
        let mut temp: Option<(PathBuf, tokio::fs::File)> = None;

        while let Some(next) = stream.next().await {
            let chunk = next.map_err(|err| {
                std::io::Error::new(std::io::ErrorKind::Other, format!("Read request body failed: {err}"))
            })?;
            len = len.saturating_add(chunk.len() as u64);

            if let Some((_, file)) = temp.as_mut() {
                file.write_all(&chunk).await?;
                continue;
            }

            if buffer.len().saturating_add(chunk.len()) <= IN_MEMORY_LIMIT_BYTES {
                buffer.extend_from_slice(&chunk);
                continue;
            }

            let (path, mut file) = create_temp_file().await?;
            if let Err(err) = file.write_all(&buffer).await {
                drop(file);
                cleanup_temp_file(&path);
                return Err(err);
            }
            buffer.clear();
            if let Err(err) = file.write_all(&chunk).await {
                drop(file);
                cleanup_temp_file(&path);
                return Err(err);
            }
            temp = Some((path, file));
        }

        if let Some((path, mut file)) = temp {
            if let Err(err) = file.flush().await {
                drop(file);
                cleanup_temp_file(&path);
                return Err(err);
            }
            return Ok(Self {
                inner: ReplayableBodyInner::TempFile { path },
                len,
            });
        }

        Ok(Self {
            inner: ReplayableBodyInner::InMemory(Bytes::from(buffer)),
            len,
        })
    }

    pub(crate) async fn read_bytes_if_small(
        &self,
        limit: usize,
    ) -> Result<Option<Bytes>, std::io::Error> {
        let Some(len) = usize::try_from(self.len).ok() else {
            return Ok(None);
        };
        if len > limit {
            return Ok(None);
        }

        match &self.inner {
            ReplayableBodyInner::InMemory(bytes) => Ok(Some(bytes.clone())),
            ReplayableBodyInner::TempFile { path } => {
                let mut file = tokio::fs::File::open(path).await?;
                let mut output = Vec::with_capacity(len);
                let mut chunk = vec![0_u8; FILE_READ_CHUNK_BYTES];
                loop {
                    let read = file.read(&mut chunk).await?;
                    if read == 0 {
                        break;
                    }
                    output.extend_from_slice(&chunk[..read]);
                }
                Ok(Some(Bytes::from(output)))
            }
        }
    }

    pub(crate) async fn to_reqwest_body(&self) -> Result<reqwest::Body, std::io::Error> {
        match &self.inner {
            ReplayableBodyInner::InMemory(bytes) => Ok(reqwest::Body::from(bytes.clone())),
            ReplayableBodyInner::TempFile { path } => {
                let file = tokio::fs::File::open(path).await?;
                Ok(reqwest::Body::from(file))
            }
        }
    }
}

impl Drop for ReplayableBody {
    fn drop(&mut self) {
        if let ReplayableBodyInner::TempFile { path } = &self.inner {
            cleanup_temp_file(path);
        }
    }
}

async fn create_temp_file() -> Result<(PathBuf, tokio::fs::File), std::io::Error> {
    let path = next_temp_path();
    let file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await?;
    Ok((path, file))
}

fn next_temp_path() -> PathBuf {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("{TEMP_FILE_PREFIX}_{now_ns}_{counter}");
    std::env::temp_dir().join(name)
}

fn cleanup_temp_file(path: &PathBuf) {
    // Prefer background cleanup to avoid blocking Tokio runtime threads on filesystem I/O.
    // Fallback to synchronous removal when no runtime is available (best-effort).
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let path = path.clone();
        handle.spawn(async move {
            let _ = tokio::fs::remove_file(&path).await;
        });
        return;
    }
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
impl ReplayableBody {
    fn is_temp_file(&self) -> bool {
        matches!(self.inner, ReplayableBodyInner::TempFile { .. })
    }

    fn temp_path(&self) -> Option<PathBuf> {
        match &self.inner {
            ReplayableBodyInner::TempFile { path } => Some(path.clone()),
            ReplayableBodyInner::InMemory(_) => None,
        }
    }
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "request_body.test.rs"]
mod tests;
