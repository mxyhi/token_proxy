use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::ProxyConfigFile;

/// 本地访问凭据与上游凭据分离；Debug 故意不派生，避免密钥进入日志。
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalApiKey {
    pub id: String,
    pub name: String,
    pub key: String,
    pub enabled: bool,
    pub scope: LocalApiKeyScope,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalApiKeyScope {
    Auto,
    Selected { upstream_ids: Vec<String> },
}

impl LocalApiKeyScope {
    pub fn allows(&self, upstream_id: &str) -> bool {
        match self {
            Self::Auto => true,
            Self::Selected { upstream_ids } => upstream_ids.iter().any(|id| id == upstream_id),
        }
    }
}

pub(crate) fn validate(keys: &[LocalApiKey]) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut secrets = HashSet::new();
    for (index, item) in keys.iter().enumerate() {
        // 错误仅包含序号，不包含凭据内容。
        if item.id.trim().is_empty() || item.name.trim().is_empty() || item.key.trim().is_empty() {
            return Err(format!(
                "local_api_keys[{index}]: id, name and key must not be empty."
            ));
        }
        if item.key.trim() != item.key || item.key.chars().any(char::is_control) {
            return Err(format!("local_api_keys[{index}]: key must not contain surrounding whitespace or control characters."));
        }
        if !ids.insert(&item.id) || !secrets.insert(&item.key) {
            return Err(format!("local_api_keys[{index}]: duplicate id or key."));
        }
        if let LocalApiKeyScope::Selected { upstream_ids } = &item.scope {
            let unique: HashSet<_> = upstream_ids.iter().collect();
            if upstream_ids.is_empty()
                || upstream_ids.iter().any(|id| id.trim().is_empty())
                || unique.len() != upstream_ids.len()
            {
                return Err(format!(
                    "local_api_keys[{index}]: select at least one unique, nonempty upstream id."
                ));
            }
            // 已删除的上游绑定必须保留：禁止悄悄回退 Auto 或扩大范围。
        }
    }
    Ok(())
}

pub(crate) fn migrate(root: &mut Map<String, Value>) -> Result<bool, String> {
    if !root.contains_key("local_api_key") {
        return Ok(false);
    }
    if root.contains_key("local_api_keys") {
        return Err("local_api_key and local_api_keys cannot coexist.".into());
    }
    let legacy = root.get("local_api_key").expect("checked presence");
    let keys = match legacy {
        Value::Null => Vec::new(),
        Value::String(key) => vec![LocalApiKey {
            id: "legacy-local-key".into(),
            name: "已迁移的 API Key".into(),
            key: key.clone(),
            enabled: true,
            scope: LocalApiKeyScope::Auto,
        }],
        _ => return Err("local_api_key must be a string or null.".into()),
    };
    validate(&keys)?;
    let migrated_key = !keys.is_empty();
    root.remove("local_api_key");
    root.insert(
        "local_api_keys".into(),
        serde_json::to_value(keys).map_err(|err| err.to_string())?,
    );
    root.insert("local_api_keys_migrated".into(), Value::Bool(migrated_key));
    Ok(true)
}

// 文件、IPC 与直接反序列化共用同一迁移/校验边界，避免旧字段被 serde 静默忽略。
impl<'de> Deserialize<'de> for ProxyConfigFile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = Value::deserialize(deserializer)?;
        if let Some(root) = value.as_object_mut() {
            migrate(root).map_err(serde::de::Error::custom)?;
        }
        let config = Self::deserialize(value).map_err(serde::de::Error::custom)?;
        validate(&config.local_api_keys).map_err(serde::de::Error::custom)?;
        Ok(config)
    }
}

impl Serialize for ProxyConfigFile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Self::serialize(self, serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key() -> Value {
        json!({"id": "first", "name": "First", "key": "test-secret", "enabled": true, "scope": {"type": "auto"}})
    }

    fn config(keys: Value) -> Value {
        json!({"host": "127.0.0.1", "port": 9208, "local_api_keys": keys})
    }

    #[test]
    fn migration_preserves_key_and_id_after_round_trip() {
        let old = json!({"host": "127.0.0.1", "port": 9208, "local_api_key": "old-secret"});
        let migrated: ProxyConfigFile = serde_json::from_value(old).unwrap();
        assert!(migrated.local_api_keys_migrated);
        assert_eq!(migrated.local_api_keys[0].key, "old-secret");
        assert!(migrated.local_api_keys[0].enabled);
        assert_eq!(migrated.local_api_keys[0].scope, LocalApiKeyScope::Auto);
        let value = serde_json::to_value(&migrated).unwrap();
        assert!(value.get("local_api_key").is_none());
        let reloaded: ProxyConfigFile = serde_json::from_value(value).unwrap();
        assert_eq!(migrated.local_api_keys[0].id, reloaded.local_api_keys[0].id);
    }

    #[test]
    fn rejects_conflict_empty_values_duplicates_and_empty_manual_scope() {
        for old in [Value::Null, json!("legacy")] {
            let mut value = config(json!([]));
            value["local_api_key"] = old;
            assert!(serde_json::from_value::<ProxyConfigFile>(value).is_err());
        }
        for field in ["id", "name", "key"] {
            let mut item = key();
            item[field] = json!(" ");
            assert!(serde_json::from_value::<ProxyConfigFile>(config(json!([item]))).is_err());
        }
        for field in ["id", "key"] {
            let first = key();
            let mut second = key();
            second["id"] = json!("second");
            second["key"] = json!("different-secret");
            second[field] = first[field].clone();
            assert!(
                serde_json::from_value::<ProxyConfigFile>(config(json!([first, second]))).is_err()
            );
        }
        let mut item = key();
        item["scope"] = json!({"type": "selected", "upstream_ids": []});
        assert!(serde_json::from_value::<ProxyConfigFile>(config(json!([item]))).is_err());
    }

    #[test]
    fn absent_and_null_legacy_stay_open_but_deleted_bindings_are_preserved() {
        for value in [
            json!({"host": "localhost", "port": 9208}),
            json!({"host": "localhost", "port": 9208, "local_api_key": null}),
        ] {
            assert!(serde_json::from_value::<ProxyConfigFile>(value)
                .unwrap()
                .local_api_keys
                .is_empty());
        }
        let mut item = key();
        item["scope"] = json!({"type": "selected", "upstream_ids": ["deleted"]});
        let config: ProxyConfigFile = serde_json::from_value(config(json!([item]))).unwrap();
        assert!(config.local_api_keys[0].scope.allows("deleted"));
        assert!(!config.local_api_keys[0].scope.allows("new-upstream"));
    }
}
