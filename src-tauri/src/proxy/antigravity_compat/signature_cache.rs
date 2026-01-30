use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const SIGNATURE_CACHE_TTL: Duration = Duration::from_secs(3 * 60 * 60);
const SIGNATURE_TEXT_HASH_LEN: usize = 16;
const MIN_VALID_SIGNATURE_LEN: usize = 50;
const GEMINI_SKIP_SENTINEL: &str = "skip_thought_signature_validator";

type Cache = HashMap<String, HashMap<String, SignatureEntry>>;

#[derive(Clone)]
struct SignatureEntry {
    signature: String,
    touched: Instant,
}

static SIGNATURE_CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache_lock() -> std::sync::MutexGuard<'static, Cache> {
    SIGNATURE_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

pub(crate) fn cache_signature(model_name: &str, text: &str, signature: &str) {
    if text.trim().is_empty() || signature.trim().is_empty() {
        return;
    }
    if signature.len() < MIN_VALID_SIGNATURE_LEN {
        return;
    }
    let group_key = model_group_key(model_name);
    let text_hash = hash_text(text);
    let mut cache = cache_lock();
    let group = cache.entry(group_key).or_insert_with(HashMap::new);
    group.insert(
        text_hash,
        SignatureEntry {
            signature: signature.to_string(),
            touched: Instant::now(),
        },
    );
}

pub(crate) fn get_cached_signature(model_name: &str, text: &str) -> String {
    let group_key = model_group_key(model_name);
    if text.trim().is_empty() {
        return fallback_signature(&group_key);
    }
    let text_hash = hash_text(text);
    let mut cache = cache_lock();
    let Some(group) = cache.get_mut(&group_key) else {
        return fallback_signature(&group_key);
    };
    let Some(entry) = group.get_mut(&text_hash) else {
        return fallback_signature(&group_key);
    };
    if entry.touched.elapsed() > SIGNATURE_CACHE_TTL {
        group.remove(&text_hash);
        return fallback_signature(&group_key);
    }
    entry.touched = Instant::now();
    entry.signature.clone()
}

pub(crate) fn has_valid_signature(model_name: &str, signature: &str) -> bool {
    if signature.trim().is_empty() {
        return false;
    }
    if signature == GEMINI_SKIP_SENTINEL {
        return model_group_key(model_name) == "gemini";
    }
    signature.len() >= MIN_VALID_SIGNATURE_LEN
}

fn fallback_signature(group_key: &str) -> String {
    if group_key == "gemini" {
        GEMINI_SKIP_SENTINEL.to_string()
    } else {
        String::new()
    }
}

fn model_group_key(model_name: &str) -> String {
    let lower = model_name.to_lowercase();
    if lower.contains("gpt") {
        return "gpt".to_string();
    }
    if lower.contains("claude") {
        return "claude".to_string();
    }
    if lower.contains("gemini") {
        return "gemini".to_string();
    }
    model_name.trim().to_string()
}

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{:x}", digest);
    hex.chars().take(SIGNATURE_TEXT_HASH_LEN).collect()
}
