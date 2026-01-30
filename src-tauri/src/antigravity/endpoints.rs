use std::env;

pub(crate) const BASE_URL_DAILY: &str = "https://daily-cloudcode-pa.googleapis.com";
pub(crate) const BASE_URL_SANDBOX: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";
pub(crate) const BASE_URL_PROD: &str = "https://cloudcode-pa.googleapis.com";

// Align with CLIProxyAPIPlus: prefer daily, then sandbox. Prod is intentionally excluded.
pub(crate) const BASE_URLS: [&str; 2] = [BASE_URL_DAILY, BASE_URL_SANDBOX];

const ANTIGRAVITY_VERSION: &str = "1.104.0";

pub(crate) fn default_user_agent() -> String {
    let os = match env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = env::consts::ARCH;
    format!("antigravity/{ANTIGRAVITY_VERSION} {os}/{arch}")
}

pub(crate) fn build_base_url_list(primary: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let primary = primary.trim_end_matches('/');
    if !primary.is_empty() {
        urls.push(primary.to_string());
    }
    for base in BASE_URLS {
        let base = base.trim_end_matches('/');
        if urls.iter().any(|value| value == base) {
            continue;
        }
        urls.push(base.to_string());
    }
    urls
}
