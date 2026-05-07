use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::collections::HashSet;
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_PRICING_VERSION: &str = "2026-05-02.openai-openrouter-v1";

const DEFAULT_LONG_CONTEXT_INPUT_TOKEN_THRESHOLD: u64 = 272_000;
const SETTINGS_ROW_ID: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingTier {
    pub input_nano_usd_per_token: u64,
    pub cached_input_nano_usd_per_token: u64,
    pub output_nano_usd_per_token: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingModel {
    pub model_id: String,
    pub aliases: Vec<String>,
    pub short: ModelPricingTier,
    pub long: Option<ModelPricingTier>,
    pub long_context_input_token_threshold: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingSettings {
    pub version: String,
    pub models: Vec<ModelPricingModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingSettingsInput {
    pub models: Vec<ModelPricingModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingSettingsSnapshot {
    pub settings: ModelPricingSettings,
    pub default_settings: ModelPricingSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PricingContextTier {
    Short,
    Long,
}

impl PricingContextTier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Long => "long",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestCost {
    pub(crate) cost_nano_usd: u64,
    pub(crate) pricing_version: String,
    pub(crate) pricing_model: String,
    pub(crate) context_tier: PricingContextTier,
}

pub fn default_model_pricing_settings() -> ModelPricingSettings {
    ModelPricingSettings {
        version: DEFAULT_PRICING_VERSION.to_string(),
        models: vec![
            ModelPricingModel {
                model_id: "gpt-5.5".to_string(),
                aliases: vec!["gpt-5.5".to_string(), "openai/gpt-5.5".to_string()],
                short: ModelPricingTier {
                    input_nano_usd_per_token: 5_000,
                    cached_input_nano_usd_per_token: 500,
                    output_nano_usd_per_token: 30_000,
                },
                long: Some(ModelPricingTier {
                    input_nano_usd_per_token: 10_000,
                    cached_input_nano_usd_per_token: 1_000,
                    output_nano_usd_per_token: 45_000,
                }),
                long_context_input_token_threshold: Some(
                    DEFAULT_LONG_CONTEXT_INPUT_TOKEN_THRESHOLD,
                ),
            },
            ModelPricingModel {
                model_id: "gpt-5.4".to_string(),
                aliases: vec!["gpt-5.4".to_string(), "openai/gpt-5.4".to_string()],
                short: ModelPricingTier {
                    input_nano_usd_per_token: 2_500,
                    cached_input_nano_usd_per_token: 250,
                    output_nano_usd_per_token: 15_000,
                },
                long: Some(ModelPricingTier {
                    input_nano_usd_per_token: 5_000,
                    cached_input_nano_usd_per_token: 500,
                    output_nano_usd_per_token: 22_500,
                }),
                long_context_input_token_threshold: Some(
                    DEFAULT_LONG_CONTEXT_INPUT_TOKEN_THRESHOLD,
                ),
            },
            ModelPricingModel {
                model_id: "gpt-5.4-mini".to_string(),
                aliases: vec![
                    "gpt-5.4-mini".to_string(),
                    "openai/gpt-5.4-mini".to_string(),
                ],
                short: ModelPricingTier {
                    input_nano_usd_per_token: 750,
                    cached_input_nano_usd_per_token: 75,
                    output_nano_usd_per_token: 4_500,
                },
                long: None,
                long_context_input_token_threshold: None,
            },
            ModelPricingModel {
                model_id: "gpt-image-2".to_string(),
                aliases: vec!["gpt-image-2".to_string(), "openai/gpt-image-2".to_string()],
                short: ModelPricingTier {
                    input_nano_usd_per_token: 8_000,
                    cached_input_nano_usd_per_token: 2_000,
                    output_nano_usd_per_token: 30_000,
                },
                long: None,
                long_context_input_token_threshold: None,
            },
            ModelPricingModel {
                model_id: "gpt-5.4-image-2".to_string(),
                aliases: vec![
                    "gpt-5.4-image-2".to_string(),
                    "openai/gpt-5.4-image-2".to_string(),
                ],
                short: ModelPricingTier {
                    input_nano_usd_per_token: 8_000,
                    cached_input_nano_usd_per_token: 2_000,
                    output_nano_usd_per_token: 15_000,
                },
                long: None,
                long_context_input_token_threshold: None,
            },
        ],
    }
}

pub fn default_model_pricing_settings_snapshot() -> ModelPricingSettingsSnapshot {
    let default_settings = default_model_pricing_settings();
    ModelPricingSettingsSnapshot {
        settings: default_settings.clone(),
        default_settings,
    }
}

pub async fn init_model_pricing_table(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS model_pricing_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  version TEXT NOT NULL,
  models_json TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create model_pricing_settings table: {err}"))?;
    Ok(())
}

pub async fn read_model_pricing_settings_snapshot(
    pool: &SqlitePool,
) -> Result<ModelPricingSettingsSnapshot, String> {
    let settings = read_model_pricing_settings(pool).await?;
    Ok(ModelPricingSettingsSnapshot {
        settings,
        default_settings: default_model_pricing_settings(),
    })
}

pub async fn read_model_pricing_settings(
    pool: &SqlitePool,
) -> Result<ModelPricingSettings, String> {
    init_model_pricing_table(pool).await?;
    let row = sqlx::query(
        r#"
SELECT models_json
FROM model_pricing_settings
WHERE id = ?;
"#,
    )
    .bind(SETTINGS_ROW_ID)
    .fetch_optional(pool)
    .await
    .map_err(|err| format!("Failed to read model pricing settings: {err}"))?;

    let Some(row) = row else {
        return Ok(default_model_pricing_settings());
    };
    let models_json = row
        .try_get::<String, _>("models_json")
        .map_err(|err| format!("Failed to decode model pricing settings: {err}"))?;
    let models = serde_json::from_str::<Vec<ModelPricingModel>>(&models_json)
        .map_err(|err| format!("Failed to parse model pricing settings: {err}"))?;
    normalize_model_pricing_settings(ModelPricingSettingsInput { models })
}

pub async fn save_model_pricing_settings(
    pool: &SqlitePool,
    input: ModelPricingSettingsInput,
) -> Result<ModelPricingSettingsSnapshot, String> {
    init_model_pricing_table(pool).await?;
    let settings = normalize_model_pricing_settings(input)?;
    if settings.models == default_model_pricing_settings().models {
        sqlx::query("DELETE FROM model_pricing_settings WHERE id = ?;")
            .bind(SETTINGS_ROW_ID)
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to reset default model pricing settings: {err}"))?;
    } else {
        let models_json = serde_json::to_string(&settings.models)
            .map_err(|err| format!("Failed to serialize model pricing settings: {err}"))?;
        sqlx::query(
            r#"
INSERT INTO model_pricing_settings (id, version, models_json, updated_at_ms)
VALUES (?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
  version = excluded.version,
  models_json = excluded.models_json,
  updated_at_ms = excluded.updated_at_ms;
"#,
        )
        .bind(SETTINGS_ROW_ID)
        .bind(&settings.version)
        .bind(models_json)
        .bind(now_ms_i64())
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to save model pricing settings: {err}"))?;
    }
    backfill_request_log_costs_with_settings(pool, &settings).await?;
    read_model_pricing_settings_snapshot(pool).await
}

pub async fn reset_model_pricing_settings(
    pool: &SqlitePool,
) -> Result<ModelPricingSettingsSnapshot, String> {
    init_model_pricing_table(pool).await?;
    sqlx::query("DELETE FROM model_pricing_settings WHERE id = ?;")
        .bind(SETTINGS_ROW_ID)
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to reset model pricing settings: {err}"))?;
    let settings = default_model_pricing_settings();
    backfill_request_log_costs_with_settings(pool, &settings).await?;
    read_model_pricing_settings_snapshot(pool).await
}

pub(crate) async fn backfill_request_log_costs(pool: &SqlitePool) -> Result<(), String> {
    let settings = read_model_pricing_settings(pool).await?;
    backfill_request_log_costs_with_settings(pool, &settings).await
}

pub(crate) fn calculate_request_cost(
    settings: &ModelPricingSettings,
    model: Option<&str>,
    mapped_model: Option<&str>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_tokens: Option<u64>,
) -> Option<RequestCost> {
    let effective_model = mapped_model
        .and_then(non_empty)
        .or_else(|| model.and_then(non_empty))?;
    let price = find_model_price(settings, effective_model)?;
    let input_tokens = input_tokens?;
    let output_tokens = output_tokens.unwrap_or(0);
    let cached_tokens = cached_tokens.unwrap_or(0).min(input_tokens);
    let uncached_input_tokens = input_tokens.saturating_sub(cached_tokens);
    let has_long_tier = price.long.is_some();
    let long_threshold = price
        .long_context_input_token_threshold
        .unwrap_or(DEFAULT_LONG_CONTEXT_INPUT_TOKEN_THRESHOLD);
    let context_tier = if has_long_tier && input_tokens > long_threshold {
        PricingContextTier::Long
    } else {
        PricingContextTier::Short
    };
    let tier = match context_tier {
        PricingContextTier::Long => price.long.as_ref().unwrap_or(&price.short),
        PricingContextTier::Short => &price.short,
    };

    Some(RequestCost {
        cost_nano_usd: uncached_input_tokens
            .saturating_mul(tier.input_nano_usd_per_token)
            .saturating_add(cached_tokens.saturating_mul(tier.cached_input_nano_usd_per_token))
            .saturating_add(output_tokens.saturating_mul(tier.output_nano_usd_per_token)),
        pricing_version: settings.version.clone(),
        pricing_model: price.model_id.clone(),
        context_tier,
    })
}

fn normalize_model_pricing_settings(
    input: ModelPricingSettingsInput,
) -> Result<ModelPricingSettings, String> {
    if input.models.is_empty() {
        return Err("At least one model price is required.".to_string());
    }

    let mut normalized_aliases = HashSet::new();
    let mut models = Vec::with_capacity(input.models.len());
    for model in input.models {
        let model_id = model.model_id.trim();
        if model_id.is_empty() {
            return Err("Model id is required.".to_string());
        }

        let mut aliases = Vec::new();
        push_alias(&mut aliases, model_id);
        for alias in model.aliases {
            push_alias(&mut aliases, &alias);
        }
        for alias in &aliases {
            let normalized = normalize_model_alias(alias);
            if !normalized_aliases.insert(normalized) {
                return Err(format!("Duplicate model pricing alias: {alias}"));
            }
        }

        let long_context_input_token_threshold = if model.long.is_some() {
            Some(
                model
                    .long_context_input_token_threshold
                    .unwrap_or(DEFAULT_LONG_CONTEXT_INPUT_TOKEN_THRESHOLD),
            )
        } else {
            None
        };

        models.push(ModelPricingModel {
            model_id: model_id.to_string(),
            aliases,
            short: model.short,
            long: model.long,
            long_context_input_token_threshold,
        });
    }

    let default_settings = default_model_pricing_settings();
    let version = if models == default_settings.models {
        DEFAULT_PRICING_VERSION.to_string()
    } else {
        pricing_version_for_models(&models)?
    };

    Ok(ModelPricingSettings { version, models })
}

fn push_alias(aliases: &mut Vec<String>, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    let normalized = normalize_model_alias(trimmed);
    if aliases
        .iter()
        .any(|existing| normalize_model_alias(existing) == normalized)
    {
        return;
    }
    aliases.push(trimmed.to_string());
}

fn pricing_version_for_models(models: &[ModelPricingModel]) -> Result<String, String> {
    let json = serde_json::to_vec(models)
        .map_err(|err| format!("Failed to serialize model pricing version input: {err}"))?;
    let digest = Sha256::digest(json);
    let mut suffix = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        write!(&mut suffix, "{byte:02x}")
            .map_err(|err| format!("Failed to format model pricing version: {err}"))?;
    }
    Ok(format!("custom.{suffix}"))
}

async fn backfill_request_log_costs_with_settings(
    pool: &SqlitePool,
    settings: &ModelPricingSettings,
) -> Result<(), String> {
    let rows = sqlx::query(
        r#"
SELECT
  id,
  model,
  mapped_model,
  input_tokens,
  output_tokens,
  cached_tokens
FROM request_logs
WHERE pricing_version IS NULL OR pricing_version != ?;
"#,
    )
    .bind(&settings.version)
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to read request log costs for backfill: {err}"))?;

    if rows.is_empty() {
        return Ok(());
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|err| format!("Failed to begin request log cost backfill transaction: {err}"))?;

    for row in rows {
        let id = row
            .try_get::<i64, _>("id")
            .map_err(|err| format!("Failed to decode request_logs.id: {err}"))?;
        let model = row.try_get::<Option<String>, _>("model").ok().flatten();
        let mapped_model = row
            .try_get::<Option<String>, _>("mapped_model")
            .ok()
            .flatten();
        let input_tokens = row
            .try_get::<Option<i64>, _>("input_tokens")
            .ok()
            .flatten()
            .and_then(i64_to_u64);
        let output_tokens = row
            .try_get::<Option<i64>, _>("output_tokens")
            .ok()
            .flatten()
            .and_then(i64_to_u64);
        let cached_tokens = row
            .try_get::<Option<i64>, _>("cached_tokens")
            .ok()
            .flatten()
            .and_then(i64_to_u64);
        let cost = calculate_request_cost(
            settings,
            model.as_deref(),
            mapped_model.as_deref(),
            input_tokens,
            output_tokens,
            cached_tokens,
        );

        sqlx::query(
            r#"
UPDATE request_logs
SET
  cost_nano_usd = ?,
  pricing_version = ?,
  pricing_model = ?,
  pricing_context_tier = ?
WHERE id = ?;
"#,
        )
        .bind(
            cost.as_ref()
                .map(|cost| i64::try_from(cost.cost_nano_usd).unwrap_or(i64::MAX)),
        )
        .bind(&settings.version)
        .bind(cost.as_ref().map(|cost| cost.pricing_model.as_str()))
        .bind(cost.as_ref().map(|cost| cost.context_tier.as_str()))
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|err| format!("Failed to backfill request log cost for {id}: {err}"))?;
    }

    tx.commit()
        .await
        .map_err(|err| format!("Failed to commit request log cost backfill: {err}"))
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn find_model_price<'a>(
    settings: &'a ModelPricingSettings,
    model: &str,
) -> Option<&'a ModelPricingModel> {
    let normalized = normalize_model_alias(model);
    settings.models.iter().find(|price| {
        normalize_model_alias(&price.model_id) == normalized
            || price
                .aliases
                .iter()
                .any(|alias| normalize_model_alias(alias) == normalized)
    })
}

fn normalize_model_alias(model: &str) -> String {
    model
        .trim()
        .to_ascii_lowercase()
        .replace(char::is_whitespace, "-")
}

fn now_ms_i64() -> i64 {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn i64_to_u64(value: i64) -> Option<u64> {
    u64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{sqlite::SqlitePoolOptions, Row};

    fn custom_settings_input() -> ModelPricingSettingsInput {
        ModelPricingSettingsInput {
            models: vec![ModelPricingModel {
                model_id: "custom-model".to_string(),
                aliases: vec!["openai/custom-model".to_string()],
                short: ModelPricingTier {
                    input_nano_usd_per_token: 100,
                    cached_input_nano_usd_per_token: 10,
                    output_nano_usd_per_token: 200,
                },
                long: Some(ModelPricingTier {
                    input_nano_usd_per_token: 300,
                    cached_input_nano_usd_per_token: 30,
                    output_nano_usd_per_token: 400,
                }),
                long_context_input_token_threshold: Some(1_000),
            }],
        }
    }

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        crate::proxy::sqlite::init_schema(&pool)
            .await
            .expect("init sqlite");
        pool
    }

    #[test]
    fn calculates_short_context_request_cost() {
        let settings = default_model_pricing_settings();
        let cost = calculate_request_cost(
            &settings,
            Some("openai/gpt-5.5"),
            None,
            Some(200_000),
            Some(10_000),
            Some(20_000),
        )
        .expect("cost");

        assert_eq!(cost.cost_nano_usd, 1_210_000_000);
        assert_eq!(cost.pricing_version, DEFAULT_PRICING_VERSION);
        assert_eq!(cost.pricing_model, "gpt-5.5");
        assert_eq!(cost.context_tier, PricingContextTier::Short);
    }

    #[test]
    fn calculates_long_context_request_cost_from_mapped_model() {
        let settings = default_model_pricing_settings();
        let cost = calculate_request_cost(
            &settings,
            Some("alias"),
            Some("gpt-5.4"),
            Some(1_000_000),
            Some(10_000),
            Some(200_000),
        )
        .expect("cost");

        assert_eq!(cost.cost_nano_usd, 4_325_000_000);
        assert_eq!(cost.pricing_model, "gpt-5.4");
        assert_eq!(cost.context_tier, PricingContextTier::Long);
    }

    #[test]
    fn leaves_unknown_or_missing_usage_unpriced() {
        let settings = default_model_pricing_settings();
        assert!(calculate_request_cost(
            &settings,
            Some("unknown-model"),
            None,
            Some(1),
            Some(1),
            None
        )
        .is_none());
        assert!(
            calculate_request_cost(&settings, Some("gpt-5.5"), None, None, Some(1), None).is_none()
        );
    }

    #[test]
    fn normalizes_custom_settings_and_prices_cached_tokens() {
        let settings = normalize_model_pricing_settings(custom_settings_input()).expect("settings");
        assert!(settings.version.starts_with("custom."));

        let cost = calculate_request_cost(
            &settings,
            Some("openai/custom-model"),
            None,
            Some(100),
            Some(10),
            Some(20),
        )
        .expect("cost");

        assert_eq!(cost.cost_nano_usd, 10_200);
        assert_eq!(cost.pricing_model, "custom-model");
        assert_eq!(cost.context_tier, PricingContextTier::Short);
    }

    #[test]
    fn rejects_duplicate_aliases() {
        let result = normalize_model_pricing_settings(ModelPricingSettingsInput {
            models: vec![
                ModelPricingModel {
                    model_id: "left".to_string(),
                    aliases: vec!["shared".to_string()],
                    short: ModelPricingTier {
                        input_nano_usd_per_token: 1,
                        cached_input_nano_usd_per_token: 1,
                        output_nano_usd_per_token: 1,
                    },
                    long: None,
                    long_context_input_token_threshold: None,
                },
                ModelPricingModel {
                    model_id: "right".to_string(),
                    aliases: vec![" Shared ".to_string()],
                    short: ModelPricingTier {
                        input_nano_usd_per_token: 1,
                        cached_input_nano_usd_per_token: 1,
                        output_nano_usd_per_token: 1,
                    },
                    long: None,
                    long_context_input_token_threshold: None,
                },
            ],
        });

        assert!(result
            .expect_err("duplicate alias")
            .to_ascii_lowercase()
            .contains("shared"));
    }

    #[tokio::test]
    async fn reads_default_settings_when_no_custom_row_exists() {
        let pool = setup_pool().await;

        let snapshot = read_model_pricing_settings_snapshot(&pool)
            .await
            .expect("snapshot");

        assert_eq!(snapshot.settings, snapshot.default_settings);
        assert_eq!(snapshot.settings.version, DEFAULT_PRICING_VERSION);
    }

    #[tokio::test]
    async fn save_model_pricing_settings_backfills_request_logs() {
        let pool = setup_pool().await;
        sqlx::query(
            r#"
INSERT INTO request_logs (
  ts_ms,
  path,
  provider,
  upstream_id,
  model,
  mapped_model,
  stream,
  status,
  input_tokens,
  output_tokens,
  total_tokens,
  cached_tokens,
  latency_ms,
  pricing_version
)
VALUES (1, '/v1/chat/completions', 'openai', 'test', 'openai/custom-model', NULL, 0, 200, 100, 10, 110, 20, 30, 'old');
"#,
        )
        .execute(&pool)
        .await
        .expect("insert request log");

        let snapshot = save_model_pricing_settings(&pool, custom_settings_input())
            .await
            .expect("save settings");

        let row = sqlx::query(
            r#"
SELECT cost_nano_usd, pricing_version, pricing_model, pricing_context_tier
FROM request_logs
WHERE id = 1;
"#,
        )
        .fetch_one(&pool)
        .await
        .expect("request log");

        assert!(snapshot.settings.version.starts_with("custom."));
        assert_eq!(row.try_get::<i64, _>("cost_nano_usd").ok(), Some(10_200));
        assert_eq!(
            row.try_get::<String, _>("pricing_version").ok().as_deref(),
            Some(snapshot.settings.version.as_str())
        );
        assert_eq!(
            row.try_get::<String, _>("pricing_model").ok().as_deref(),
            Some("custom-model")
        );
        assert_eq!(
            row.try_get::<String, _>("pricing_context_tier")
                .ok()
                .as_deref(),
            Some("short")
        );
    }
}
