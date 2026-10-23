//! 精确定位上游明确拒绝的 reasoning；任何歧义保持原失败，绝不批量删除上下文。
use super::{allowed_repair, RequestRepair, RequestRepairKind, RequestRepairState};
use crate::proxy::request_body::ReplayableBody;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::{collections::HashSet, fmt};

pub(in crate::proxy::upstream) fn repair_codex_encrypted_body(
    request: &ReplayableBody,
    rejection: &[u8],
    state: &mut RequestRepairState,
) -> Result<Option<RequestRepair>, String> {
    if rejection.len() > 65536 || !unambiguous_json(rejection) {
        return Ok(None);
    }
    let Ok(error) = serde_json::from_slice::<Value>(rejection) else {
        return Ok(None);
    };
    let Some(source) = error_source(&error) else {
        return Ok(None);
    };
    if source.get("code").and_then(Value::as_str) != Some("invalid_encrypted_content") {
        return Ok(None);
    }
    let Some(message) = source.get("message").and_then(Value::as_str) else {
        return Ok(None);
    };
    let generic = matches!(message,
        "The encrypted content could not be verified" |
        "The encrypted content could not be verified." |
        "The encrypted content could not be verified. Reason: Encrypted content could not be decrypted or parsed.");
    let locator = message.strip_prefix("The encrypted content ")
        .and_then(|message| message.strip_suffix(" could not be verified. Reason: Encrypted content could not be decrypted or parsed."))
        .and_then(|locator| locator.split_once("..."))
        .filter(|(start,end)| [*start,*end].iter().all(|part| part.len()==4 && part.bytes().all(|b| b.is_ascii_alphanumeric() || b==b'_' || b==b'-')));
    if !generic && locator.is_none() {
        return Ok(None);
    }
    if !unambiguous_json(request.as_bytes()) {
        return Ok(None);
    }
    let Ok(mut request) = serde_json::from_slice::<Value>(request.as_bytes()) else {
        return Ok(None);
    };
    let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(None);
    };
    let mut ids = HashSet::new();
    let mut candidates = Vec::new();
    for (index, item) in input.iter().enumerate() {
        let Some(item) = item.as_object() else {
            return Ok(None);
        };
        let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if matches!(kind, "compaction" | "compaction_summary" | "agent_message") {
            return Ok(None);
        }
        if let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            if !ids.insert(id) {
                return Ok(None);
            }
        }
        if let Some(encrypted) = item.get("encrypted_content").filter(|v| !v.is_null()) {
            let Some(encrypted) = encrypted.as_str().filter(|v| !v.is_empty()) else {
                return Ok(None);
            };
            if kind != "reasoning" {
                return Ok(None);
            }
            if locator
                .is_none_or(|(start, end)| encrypted.starts_with(start) && encrypted.ends_with(end))
            {
                candidates.push(index);
            }
        }
    }
    if candidates.len() != 1 {
        return Ok(None);
    }
    input.remove(candidates[0]);
    let body = serde_json::to_vec(&request)
        .map_err(|_| "Failed to encode repaired request".to_string())?;
    allowed_repair(
        state,
        RequestRepairKind::CodexEncryptedContent,
        body,
        "confirmed rejected reasoning item",
    )
}

fn error_source(value: &Value) -> Option<&Value> {
    let nested = value.pointer("/response/error");
    let error = value.get("error");
    if nested.is_some() && error.is_some() {
        return None;
    }
    if (nested.is_some() || error.is_some())
        && (value.get("code").is_some() || value.get("message").is_some())
    {
        return None;
    }
    nested
        .or(error)
        .or(Some(value))
        .filter(|value| value.is_object())
}

// serde_json normally accepts duplicate object keys. Recovery changes a request, so ambiguity is unsafe.
fn unambiguous_json(bytes: &[u8]) -> bool {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    UniqueJson::deserialize(&mut decoder).is_ok() && decoder.end().is_ok()
}
struct UniqueJson;
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unambiguous JSON")
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_unit<E: de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<UniqueJson, A::Error> {
                while seq.next_element::<UniqueJson>()?.is_some() {}
                Ok(UniqueJson)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<UniqueJson, A::Error> {
                let mut keys = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                    map.next_value::<UniqueJson>()?;
                }
                Ok(UniqueJson)
            }
        }
        d.deserialize_any(UniqueVisitor)
    }
}
