use axum::body::{Body, Bytes};
use futures_util::StreamExt;

// 将入站请求体缓存为“可重放”形式，便于上游重试/降级时重复发送同一份请求体。
#[derive(Clone)]
pub(crate) struct ReplayableBody {
    bytes: Bytes,
}

impl ReplayableBody {
    pub(crate) fn from_bytes(bytes: Bytes) -> Self {
        Self { bytes }
    }

    pub(crate) async fn from_body(body: Body) -> Result<Self, std::io::Error> {
        let mut stream = body.into_data_stream();
        let mut buffer: Vec<u8> = Vec::new();

        while let Some(next) = stream.next().await {
            let chunk = next.map_err(|err| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Read request body failed: {err}"),
                )
            })?;
            buffer.extend_from_slice(&chunk);
        }

        Ok(Self {
            bytes: Bytes::from(buffer),
        })
    }

    pub(crate) async fn read_bytes_if_small(
        &self,
        limit: usize,
    ) -> Result<Option<Bytes>, std::io::Error> {
        if self.bytes.len() > limit {
            return Ok(None);
        }

        Ok(Some(self.bytes.clone()))
    }

    pub(crate) async fn to_reqwest_body(&self) -> Result<reqwest::Body, std::io::Error> {
        Ok(reqwest::Body::from(self.bytes.clone()))
    }
}

#[cfg(test)]
impl ReplayableBody {
    fn is_temp_file(&self) -> bool {
        false
    }
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
mod tests;
