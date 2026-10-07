//! 有界响应详情捕获：小正文留在内存，大正文写入关闭即删除的匿名文件。
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufWriter};

pub const BODY_MEMORY_LIMIT: usize = 256 * 1024;
pub const BODY_PAGE_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub struct ResponseBodyCapture {
    enabled: bool,
    memory: Vec<u8>,
    file: Option<BufWriter<File>>,
    migrated: usize,
    pending: bool,
    error: Option<String>,
}

impl ResponseBodyCapture {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub fn take(&mut self) -> Self {
        std::mem::take(self)
    }

    pub async fn push(&mut self, bytes: &[u8]) {
        if !self.enabled || self.error.is_some() {
            return;
        }
        if self.pending {
            self.fail("Response detail capture was interrupted");
            return;
        }
        if self.file.is_none() && self.memory.len().saturating_add(bytes.len()) <= BODY_MEMORY_LIMIT
        {
            let needed = self.memory.len() + bytes.len();
            if self.memory.capacity() < needed {
                let capacity = needed
                    .max(self.memory.capacity().saturating_mul(2))
                    .min(BODY_MEMORY_LIMIT);
                self.memory.reserve_exact(capacity - self.memory.len());
            }
            self.memory.extend_from_slice(bytes);
            return;
        }
        // 每个 await 前保留可见状态。future 被取消后，已捕获前缀仍属于此对象。
        self.pending = true;
        if self.file.is_none() {
            match tokio::task::spawn_blocking(tempfile::tempfile).await {
                Ok(Ok(file)) => {
                    self.file = Some(BufWriter::with_capacity(
                        BODY_PAGE_BYTES,
                        File::from_std(file),
                    ))
                }
                _ => {
                    self.fail("Failed to create response detail temporary file");
                    self.pending = false;
                    return;
                }
            }
        }
        if !self.migrate_memory().await {
            self.pending = false;
            return;
        }
        let mut offset = 0;
        while offset < bytes.len() {
            let end = (offset + BODY_PAGE_BYTES).min(bytes.len());
            match self.file.as_mut().unwrap().write(&bytes[offset..end]).await {
                Ok(0) | Err(_) => {
                    self.fail("Failed to write response detail temporary file");
                    break;
                }
                Ok(count) => offset += count,
            }
        }
        self.pending = false;
    }

    fn fail(&mut self, message: &str) {
        if self.error.is_none() {
            self.error = Some(message.to_owned());
        }
    }

    async fn migrate_memory(&mut self) -> bool {
        while self.migrated < self.memory.len() {
            let end = (self.migrated + BODY_PAGE_BYTES).min(self.memory.len());
            match self
                .file
                .as_mut()
                .unwrap()
                .write(&self.memory[self.migrated..end])
                .await
            {
                Ok(0) | Err(_) => {
                    self.fail("Failed to write response detail temporary file");
                    // 新正文尚未追加；原内存仍是完整前缀，丢弃文件中的重复片段。
                    self.file = None;
                    self.migrated = 0;
                    return false;
                }
                Ok(count) => self.migrated += count,
            }
        }
        self.memory = Vec::new();
        self.migrated = 0;
        true
    }

    pub(crate) async fn finish(mut self) -> CapturedBody {
        if self.pending {
            self.fail("Response detail capture was interrupted");
        }
        if self.file.is_some() {
            // 被取消的迁移必须补完原先已接收的内存前缀。
            self.migrate_memory().await;
        }
        if let Some(mut writer) = self.file.take() {
            if writer.flush().await.is_err() {
                self.fail("Failed to flush response detail temporary file");
            }
            let mut file = writer.into_inner();
            if file.seek(std::io::SeekFrom::Start(0)).await.is_err() {
                self.fail("Failed to read response detail temporary file");
                return CapturedBody {
                    memory: Vec::new(),
                    file: None,
                    error: self.error,
                };
            }
            CapturedBody {
                memory: Vec::new(),
                file: Some(file),
                error: self.error,
            }
        } else {
            CapturedBody {
                memory: self.memory,
                file: None,
                error: self.error,
            }
        }
    }
}

pub(crate) struct CapturedBody {
    pub memory: Vec<u8>,
    pub file: Option<File>,
    pub error: Option<String>,
}

impl CapturedBody {
    pub async fn read_chunk(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let Some(file) = self.file.as_mut() else {
            return Ok(0);
        };
        // 固定大小填满后再写库，使 ordinal 可以直接映射分页字节偏移。
        let mut count = 0;
        while count < buffer.len() {
            match file.read(&mut buffer[count..]).await {
                Ok(0) => break,
                Ok(read) => count += read,
                Err(error) => {
                    self.error.get_or_insert_with(|| {
                        "Failed to read response detail temporary file".to_owned()
                    });
                    self.file = None;
                    if count == 0 {
                        return Err(error);
                    }
                    // 本块读取中途失败时仍提交已读取前缀，错误单独持久化。
                    break;
                }
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn failed_migration_preserves_entire_existing_memory_prefix() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let readonly = std::fs::File::open(temp.path()).unwrap();
        let mut capture = ResponseBodyCapture::new(true);
        let prefix = vec![b'p'; BODY_MEMORY_LIMIT];
        capture.push(&prefix).await;
        capture.file = Some(BufWriter::with_capacity(
            BODY_PAGE_BYTES,
            File::from_std(readonly),
        ));
        capture.push(b"not captured").await;
        let result = capture.finish().await;
        assert_eq!(result.memory, prefix);
        assert!(result.file.is_none());
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn memory_capacity_never_exceeds_capture_threshold() {
        let mut capture = ResponseBodyCapture::new(true);
        capture.push(&vec![b'x'; 200_000]).await;
        capture.push(&vec![b'y'; 60_000]).await;
        assert!(capture.memory.capacity() <= BODY_MEMORY_LIMIT);
        assert_eq!(capture.memory.len(), 260_000);
    }

    #[tokio::test]
    async fn cancelled_push_retains_prefix_and_marks_capture_error() {
        use std::future::Future;
        use std::task::Poll;
        let mut capture = ResponseBodyCapture::new(true);
        let prefix = vec![b'p'; BODY_MEMORY_LIMIT];
        capture.push(&prefix).await;
        let mut push = Box::pin(capture.push(b"new bytes"));
        let pending =
            std::future::poll_fn(|cx| Poll::Ready(push.as_mut().poll(cx).is_pending())).await;
        drop(push);
        let mut result = capture.finish().await;
        let mut actual = result.memory;
        if let Some(file) = result.file.as_mut() {
            file.read_to_end(&mut actual).await.unwrap();
        }
        assert!(actual.starts_with(&prefix));
        if pending {
            assert!(result.error.is_some());
        }
    }

    #[tokio::test]
    async fn disk_write_failure_is_explicit_and_stops_capture() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let readonly = std::fs::File::open(temp.path()).unwrap();
        let mut capture = ResponseBodyCapture::new(true);
        capture.file = Some(BufWriter::with_capacity(
            BODY_PAGE_BYTES,
            File::from_std(readonly),
        ));
        capture.push(&vec![b'a'; BODY_PAGE_BYTES * 3]).await;
        capture.push(b"must not resume after error").await;
        let result = capture.finish().await;
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn capture_spills_without_losing_bytes() {
        let mut capture = ResponseBodyCapture::new(true);
        capture.push(&vec![b'a'; BODY_MEMORY_LIMIT]).await;
        assert!(capture.file.is_none());
        capture.push("中文".as_bytes()).await;
        assert!(capture.memory.is_empty());
        let mut result = capture.take().finish().await;
        let mut actual = Vec::new();
        result
            .file
            .as_mut()
            .unwrap()
            .read_to_end(&mut actual)
            .await
            .unwrap();
        assert_eq!(actual.len(), BODY_MEMORY_LIMIT + 6);
        assert_eq!(&actual[BODY_MEMORY_LIMIT..], "中文".as_bytes());
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn interrupted_migration_preserves_previously_captured_prefix() {
        let mut capture = ResponseBodyCapture::new(true);
        capture.push(b"preserved").await;
        capture.pending = true;
        let result = capture.finish().await;
        assert_eq!(result.memory, b"preserved");
        assert!(result.error.unwrap().contains("interrupted"));
    }

    #[tokio::test]
    async fn disabled_capture_does_not_retain_body() {
        let mut capture = ResponseBodyCapture::new(false);
        capture.push(&vec![0; BODY_MEMORY_LIMIT + 1]).await;
        let result = capture.finish().await;
        assert!(result.memory.is_empty());
        assert!(result.file.is_none());
    }
}
