use serde::{Deserialize, Serialize};
use tracing_subscriber::{
    fmt, layer::SubscriberExt, reload, util::SubscriberInitExt, EnvFilter, Registry,
};

const DEFAULT_DIRECTIVE: &str = "off";

/// 全局 tracing 日志等级；silent 表示完全禁用。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Silent,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Default for LogLevel {
    fn default() -> Self {
        Self::Silent
    }
}

impl LogLevel {
    fn as_directive(self) -> &'static str {
        match self {
            LogLevel::Silent => DEFAULT_DIRECTIVE,
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

#[derive(Clone, Default)]
pub struct LoggingState {
    handle: Option<reload::Handle<EnvFilter, Registry>>,
}

impl LoggingState {
    /// 初始化全局 tracing。若已存在全局订阅者则静默跳过。
    pub fn init(initial_level: LogLevel) -> Self {
        let level = if cfg!(debug_assertions) {
            initial_level
        } else {
            LogLevel::Silent
        };
        let filter = EnvFilter::new(level.as_directive());
        let (filter_layer, handle) = reload::Layer::new(filter);

        let subscriber = tracing_subscriber::registry().with(filter_layer).with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_file(true)
                .with_line_number(true)
                .with_writer(std::io::stderr),
        );

        let handle = match subscriber.try_init() {
            Ok(()) => Some(handle),
            Err(_err) => None, // 已初始化则保持静默；后续 reload 不可用。
        };

        Self { handle }
    }

    pub fn apply_level(&self, level: LogLevel) {
        if !cfg!(debug_assertions) {
            return;
        }
        if let Some(handle) = &self.handle {
            let _ = handle.modify(|filter| *filter = EnvFilter::new(level.as_directive()));
        }
    }
}
