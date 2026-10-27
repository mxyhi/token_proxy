use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use crate::sqlite::backfill_mark;

pub const PRICE_MULTIPLIER_SCALE: u64 = 1_000_000_000_000;
const COST_BACKFILL_MARK: &str = "request_log_cost";
/// 每批扫描的 id 窗口；单条查询与事务占用写连接的时间有上界，日志写入可穿插执行。
const COST_BACKFILL_WINDOW: i64 = 2_000;
// 启动回填、远端价格刷新和用户改价可能并发；串行执行，进度标记才对应真正跑完的价格版本，
// 启动时的用量回填也不会被费用回填插队（否则缺用量的旧日志会被算成 0 费用）。
pub(crate) static REQUEST_LOG_BACKFILL_LOCK: Mutex<()> = Mutex::const_new(());
const BUNDLED_PRICING_CATALOG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/model-pricing.json"
));
const CURATED_PRICING_OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/curated-model-pricing.json"
));
const SETTINGS_ROW_ID: i64 = 1;
// 固定默认单价（nano-USD/token），独立于任何模型的价格配置。
const FALLBACK_PRICING_PROFILE: ModelPricingProfile = ModelPricingProfile {
    input_nano_usd_per_token: Some(2_000),
    output_nano_usd_per_token: Some(10_000),
    cache_read_nano_usd_per_token: Some(100),
    cache_write_nano_usd_per_token: Some(2_500),
    cache_write_5m_nano_usd_per_token: Some(2_500),
    cache_write_1h_nano_usd_per_token: Some(4_000),
    image_input_nano_usd_per_token: None,
    image_output_nano_usd_per_token: None,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PricingSource {
    pub url: String,
    pub commit: String,
    pub sha256: String,
    pub commit_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingProfile {
    pub input_nano_usd_per_token: Option<u64>,
    pub output_nano_usd_per_token: Option<u64>,
    pub cache_read_nano_usd_per_token: Option<u64>,
    pub cache_write_nano_usd_per_token: Option<u64>,
    pub cache_write_5m_nano_usd_per_token: Option<u64>,
    pub cache_write_1h_nano_usd_per_token: Option<u64>,
    pub image_input_nano_usd_per_token: Option<u64>,
    pub image_output_nano_usd_per_token: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LongContextPricing {
    pub threshold: u64,
    pub input_multiplier_scaled: u64,
    pub output_multiplier_scaled: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingModel {
    pub model_id: String,
    pub aliases: Vec<String>,
    #[serde(default = "default_price_multiplier_scaled")]
    pub price_multiplier_scaled: u64,
    pub standard: ModelPricingProfile,
    #[serde(default)]
    pub service_tier_profiles: BTreeMap<String, ModelPricingProfile>,
    pub long_context: Option<LongContextPricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingSettings {
    pub version: String,
    pub source: Option<PricingSource>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PricingCatalog {
    schema_version: u32,
    version: String,
    source: PricingSource,
    models: Vec<ModelPricingModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CuratedPricingReference {
    pricing_url: String,
    verified_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CuratedPricingEntry {
    source: CuratedPricingReference,
    model: ModelPricingModel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CuratedPricingOverlay {
    schema_version: u32,
    version: String,
    models: Vec<CuratedPricingEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
struct ModelPricingOverrides {
    upserts: Vec<ModelPricingModel>,
    deleted_model_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PricingContextTier {
    Standard,
    Long,
}

impl PricingContextTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Long => "long",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BillableUsage {
    pub uncached_input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_write_5m_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub output_tokens: u64,
    pub image_input_tokens: u64,
    pub image_output_tokens: u64,
}

impl BillableUsage {
    pub fn total_input_tokens(&self) -> u64 {
        self.uncached_input_tokens
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
            .saturating_add(self.cache_write_5m_tokens)
            .saturating_add(self.cache_write_1h_tokens)
            .saturating_add(self.image_input_tokens)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestCostBreakdown {
    pub uncached_input_nano_usd: u64,
    pub cache_read_nano_usd: u64,
    pub cache_write_nano_usd: u64,
    pub cache_write_5m_nano_usd: u64,
    pub cache_write_1h_nano_usd: u64,
    pub output_nano_usd: u64,
    pub image_input_nano_usd: u64,
    pub image_output_nano_usd: u64,
}

impl RequestCostBreakdown {
    fn total(&self) -> u64 {
        self.uncached_input_nano_usd
            .saturating_add(self.cache_read_nano_usd)
            .saturating_add(self.cache_write_nano_usd)
            .saturating_add(self.cache_write_5m_nano_usd)
            .saturating_add(self.cache_write_1h_nano_usd)
            .saturating_add(self.output_nano_usd)
            .saturating_add(self.image_input_nano_usd)
            .saturating_add(self.image_output_nano_usd)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestCost {
    pub cost_nano_usd: u64,
    pub pricing_version: String,
    pub pricing_model: String,
    pub service_tier: String,
    pub context_tier: PricingContextTier,
    pub breakdown: RequestCostBreakdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteCatalogRefresh {
    Updated,
    NotModified,
}

static BUNDLED_SETTINGS: OnceLock<ModelPricingSettings> = OnceLock::new();
static CURATED_OVERLAY: OnceLock<CuratedPricingOverlay> = OnceLock::new();

pub fn default_model_pricing_settings() -> ModelPricingSettings {
    BUNDLED_SETTINGS
        .get_or_init(|| {
            parse_catalog(BUNDLED_PRICING_CATALOG)
                .expect("bundled model pricing catalog must be valid")
        })
        .clone()
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
CREATE TABLE IF NOT EXISTS model_pricing_catalog_cache (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  version TEXT NOT NULL,
  catalog_json TEXT NOT NULL,
  etag TEXT,
  updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS model_pricing_overrides (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  overrides_json TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
);
"#,
    )
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to create model pricing tables: {err}"))?;
    Ok(())
}

pub async fn read_model_pricing_settings_snapshot(
    pool: &SqlitePool,
) -> Result<ModelPricingSettingsSnapshot, String> {
    let default_settings = read_catalog_settings(pool).await?;
    let settings = read_effective_settings(pool, &default_settings).await?;
    Ok(ModelPricingSettingsSnapshot {
        settings,
        default_settings,
    })
}

pub async fn read_model_pricing_settings(
    pool: &SqlitePool,
) -> Result<ModelPricingSettings, String> {
    let catalog = read_catalog_settings(pool).await?;
    read_effective_settings(pool, &catalog).await
}

pub async fn save_model_pricing_settings(
    pool: &SqlitePool,
    input: ModelPricingSettingsInput,
) -> Result<ModelPricingSettingsSnapshot, String> {
    init_model_pricing_table(pool).await?;
    let catalog = read_catalog_settings(pool).await?;
    let normalized = normalize_models(input.models)?;
    let overrides = diff_overrides(&catalog.models, &normalized);
    if overrides == ModelPricingOverrides::default() {
        sqlx::query("DELETE FROM model_pricing_overrides WHERE id = ?;")
            .bind(SETTINGS_ROW_ID)
            .execute(pool)
            .await
            .map_err(|err| format!("Failed to reset model pricing overrides: {err}"))?;
    } else {
        let overrides_json = serde_json::to_string(&overrides)
            .map_err(|err| format!("Failed to serialize model pricing overrides: {err}"))?;
        sqlx::query(
            r#"
INSERT INTO model_pricing_overrides (id, overrides_json, updated_at_ms)
VALUES (?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
  overrides_json = excluded.overrides_json,
  updated_at_ms = excluded.updated_at_ms;
"#,
        )
        .bind(SETTINGS_ROW_ID)
        .bind(overrides_json)
        .bind(now_ms_i64())
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to save model pricing overrides: {err}"))?;
    }
    let settings = effective_settings(&catalog, &overrides)?;
    backfill_request_log_costs_with_settings(pool, &settings).await?;
    Ok(ModelPricingSettingsSnapshot {
        settings,
        default_settings: catalog,
    })
}

pub async fn reset_model_pricing_settings(
    pool: &SqlitePool,
) -> Result<ModelPricingSettingsSnapshot, String> {
    init_model_pricing_table(pool).await?;
    sqlx::query("DELETE FROM model_pricing_overrides WHERE id = ?;")
        .bind(SETTINGS_ROW_ID)
        .execute(pool)
        .await
        .map_err(|err| format!("Failed to reset model pricing overrides: {err}"))?;
    let settings = read_catalog_settings(pool).await?;
    backfill_request_log_costs_with_settings(pool, &settings).await?;
    Ok(ModelPricingSettingsSnapshot {
        settings: settings.clone(),
        default_settings: settings,
    })
}

/// 读取条件请求所需的远端目录 ETag；网络请求由 runtime 负责。
pub async fn read_remote_catalog_etag(pool: &SqlitePool) -> Result<Option<String>, String> {
    init_model_pricing_table(pool).await?;
    let row = sqlx::query("SELECT etag FROM model_pricing_catalog_cache WHERE id = ?;")
        .bind(SETTINGS_ROW_ID)
        .fetch_optional(pool)
        .await
        .map_err(|err| format!("Failed to read model pricing catalog ETag: {err}"))?;
    Ok(row.and_then(|row| row.try_get::<Option<String>, _>("etag").ok().flatten()))
}

/// 校验并持久化 runtime 已获取的目录，同时重算历史成本。
pub async fn store_remote_catalog(
    pool: &SqlitePool,
    catalog_json: &str,
    response_etag: Option<&str>,
) -> Result<RemoteCatalogRefresh, String> {
    init_model_pricing_table(pool).await?;
    let settings = parse_catalog(catalog_json)?;
    sqlx::query(
        r#"
INSERT INTO model_pricing_catalog_cache (id, version, catalog_json, etag, updated_at_ms)
VALUES (?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
  version = excluded.version,
  catalog_json = excluded.catalog_json,
  etag = excluded.etag,
  updated_at_ms = excluded.updated_at_ms;
"#,
    )
    .bind(SETTINGS_ROW_ID)
    .bind(&settings.version)
    .bind(catalog_json)
    .bind(response_etag)
    .bind(now_ms_i64())
    .execute(pool)
    .await
    .map_err(|err| format!("Failed to cache model pricing catalog: {err}"))?;
    let effective = read_effective_settings(pool, &settings).await?;
    backfill_request_log_costs_with_settings(pool, &effective).await?;
    tracing::info!(version = %settings.version, "model pricing catalog updated");
    Ok(RemoteCatalogRefresh::Updated)
}

/// 按当前生效价格补算费用；调用方必须已持有 [`REQUEST_LOG_BACKFILL_LOCK`]。
pub(crate) async fn backfill_request_log_costs_locked(pool: &SqlitePool) -> Result<(), String> {
    // 在锁内读取价格，排在价格刷新之后的启动回填才会用到最新版本。
    let settings = read_model_pricing_settings(pool).await?;
    backfill_request_log_costs_with_settings_locked(pool, &settings).await
}

pub fn calculate_request_cost(
    settings: &ModelPricingSettings,
    model: Option<&str>,
    mapped_model: Option<&str>,
    service_tier: Option<&str>,
    usage: &BillableUsage,
) -> Option<RequestCost> {
    let effective_model = mapped_model
        .and_then(non_empty)
        .or_else(|| model.and_then(non_empty))?;
    let model_price = find_model_price(settings, effective_model);
    if model_price.is_none() {
        tracing::debug!(
            model = effective_model,
            pricing_model = "default",
            "unmatched model uses fallback pricing"
        );
    }
    let normalized_service_tier = normalize_service_tier(service_tier);
    let selected_profile = model_price.and_then(|price| {
        price
            .service_tier_profiles
            .get(normalized_service_tier.as_str())
            .or_else(|| {
                // Fast 是 Priority 的新名称；显式配置优先，别名回退避免按 standard 少计。
                let alias = match normalized_service_tier.as_str() {
                    "fast" => "priority",
                    "priority" => "fast",
                    _ => return None,
                };
                price.service_tier_profiles.get(alias)
            })
    });
    let long_context = model_price
        .and_then(|price| price.long_context.as_ref())
        .filter(|long| usage.total_input_tokens() > long.threshold);
    let input_long_multiplier = long_context
        .map(|long| long.input_multiplier_scaled)
        .unwrap_or(PRICE_MULTIPLIER_SCALE);
    let output_long_multiplier = long_context
        .map(|long| long.output_multiplier_scaled)
        .unwrap_or(PRICE_MULTIPLIER_SCALE);
    let standard = model_price
        .map(|price| &price.standard)
        .unwrap_or(&FALLBACK_PRICING_PROFILE);
    let profile = ResolvedProfile::new(standard, selected_profile);

    let mut breakdown = RequestCostBreakdown {
        uncached_input_nano_usd: price_component(
            usage.uncached_input_tokens,
            profile.input(),
            input_long_multiplier,
        )?,
        cache_read_nano_usd: price_component(
            usage.cache_read_tokens,
            profile.cache_read(),
            input_long_multiplier,
        )?,
        cache_write_nano_usd: price_component(
            usage.cache_write_tokens,
            profile.cache_write(),
            input_long_multiplier,
        )?,
        cache_write_5m_nano_usd: price_component(
            usage.cache_write_5m_tokens,
            profile.cache_write_5m(),
            input_long_multiplier,
        )?,
        cache_write_1h_nano_usd: price_component(
            usage.cache_write_1h_tokens,
            profile.cache_write_1h(),
            input_long_multiplier,
        )?,
        output_nano_usd: price_component(
            usage.output_tokens,
            profile.output(),
            output_long_multiplier,
        )?,
        image_input_nano_usd: price_component(
            usage.image_input_tokens,
            profile.image_input(),
            PRICE_MULTIPLIER_SCALE,
        )?,
        image_output_nano_usd: price_component(
            usage.image_output_tokens,
            profile.image_output(),
            if profile.has_explicit_image_output() {
                PRICE_MULTIPLIER_SCALE
            } else {
                output_long_multiplier
            },
        )?,
    };
    apply_breakdown_multiplier(
        &mut breakdown,
        model_price.map_or(PRICE_MULTIPLIER_SCALE, |price| {
            price.price_multiplier_scaled
        }),
    );

    Some(RequestCost {
        cost_nano_usd: breakdown.total(),
        pricing_version: settings.version.clone(),
        pricing_model: model_price
            .map_or("default", |price| price.model_id.as_str())
            .to_string(),
        service_tier: normalized_service_tier,
        context_tier: if long_context.is_some() {
            PricingContextTier::Long
        } else {
            PricingContextTier::Standard
        },
        breakdown,
    })
}

struct ResolvedProfile<'a> {
    standard: &'a ModelPricingProfile,
    selected: Option<&'a ModelPricingProfile>,
}

impl<'a> ResolvedProfile<'a> {
    fn new(standard: &'a ModelPricingProfile, selected: Option<&'a ModelPricingProfile>) -> Self {
        Self { standard, selected }
    }

    fn input(&self) -> Option<u64> {
        self.selected
            .and_then(|profile| profile.input_nano_usd_per_token)
            .or(self.standard.input_nano_usd_per_token)
    }

    fn output(&self) -> Option<u64> {
        self.selected
            .and_then(|profile| profile.output_nano_usd_per_token)
            .or(self.standard.output_nano_usd_per_token)
    }

    fn cache_read(&self) -> Option<u64> {
        self.selected
            .and_then(|profile| profile.cache_read_nano_usd_per_token)
            .or(self.standard.cache_read_nano_usd_per_token)
    }

    fn cache_write(&self) -> Option<u64> {
        self.selected
            .and_then(|profile| profile.cache_write_nano_usd_per_token)
            .or(self.standard.cache_write_nano_usd_per_token)
    }

    fn cache_write_5m(&self) -> Option<u64> {
        self.selected
            .and_then(|profile| profile.cache_write_5m_nano_usd_per_token)
            .or_else(|| {
                self.selected
                    .and_then(|profile| profile.cache_write_nano_usd_per_token)
            })
            .or(self.standard.cache_write_5m_nano_usd_per_token)
            .or(self.standard.cache_write_nano_usd_per_token)
    }

    fn cache_write_1h(&self) -> Option<u64> {
        self.selected
            .and_then(|profile| profile.cache_write_1h_nano_usd_per_token)
            .or_else(|| {
                self.selected
                    .and_then(|profile| profile.cache_write_nano_usd_per_token)
            })
            .or(self.standard.cache_write_1h_nano_usd_per_token)
            .or(self.standard.cache_write_nano_usd_per_token)
    }

    fn image_input(&self) -> Option<u64> {
        self.selected
            .and_then(|profile| profile.image_input_nano_usd_per_token)
            .or(self.standard.image_input_nano_usd_per_token)
            .or_else(|| self.input())
    }

    fn image_output(&self) -> Option<u64> {
        self.selected
            .and_then(|profile| profile.image_output_nano_usd_per_token)
            .or(self.standard.image_output_nano_usd_per_token)
            .or_else(|| self.output())
    }

    fn has_explicit_image_output(&self) -> bool {
        self.selected
            .and_then(|profile| profile.image_output_nano_usd_per_token)
            .or(self.standard.image_output_nano_usd_per_token)
            .is_some()
    }
}

fn parse_catalog(json: &str) -> Result<ModelPricingSettings, String> {
    let catalog = serde_json::from_str::<PricingCatalog>(json)
        .map_err(|err| format!("Failed to parse model pricing catalog: {err}"))?;
    if catalog.schema_version != 1 {
        return Err(format!(
            "Unsupported model pricing schema version: {}",
            catalog.schema_version
        ));
    }
    if catalog.version.trim().is_empty() {
        return Err("Model pricing catalog version is required.".to_string());
    }
    let models = normalize_models(catalog.models)?;
    apply_curated_pricing_overlay(ModelPricingSettings {
        version: catalog.version,
        source: Some(catalog.source),
        models,
    })
}

// Curated official prices stay separate from the sub2api snapshot provenance.
// They are applied to every base catalog before user overrides are calculated.
fn apply_curated_pricing_overlay(
    mut settings: ModelPricingSettings,
) -> Result<ModelPricingSettings, String> {
    let overlay = curated_pricing_overlay();
    let curated_keys: HashSet<String> = overlay
        .models
        .iter()
        .flat_map(|entry| {
            std::iter::once(&entry.model.model_id)
                .chain(entry.model.aliases.iter())
                .flat_map(|model| model_lookup_keys(model))
        })
        .collect();
    settings.models.retain(|model| {
        std::iter::once(&model.model_id)
            .chain(model.aliases.iter())
            .flat_map(|model| model_lookup_keys(model))
            .all(|key| !curated_keys.contains(&key))
    });
    settings
        .models
        .extend(overlay.models.iter().map(|entry| entry.model.clone()));
    settings.models = normalize_models(settings.models)?;
    settings.version = format!("{}+{}", settings.version, overlay.version);
    tracing::debug!(
        version = %settings.version,
        curated_model_count = overlay.models.len(),
        "curated pricing overlay applied"
    );
    Ok(settings)
}

fn curated_pricing_overlay() -> &'static CuratedPricingOverlay {
    CURATED_OVERLAY.get_or_init(|| {
        let overlay = serde_json::from_str::<CuratedPricingOverlay>(CURATED_PRICING_OVERLAY)
            .expect("bundled curated pricing overlay must be valid JSON");
        assert_eq!(
            overlay.schema_version, 1,
            "bundled curated pricing overlay schema must be supported"
        );
        assert!(
            !overlay.version.trim().is_empty(),
            "bundled curated pricing overlay version is required"
        );
        assert!(
            !overlay.models.is_empty(),
            "bundled curated pricing overlay requires models"
        );
        for entry in &overlay.models {
            assert!(
                entry.source.pricing_url.starts_with("https://"),
                "curated pricing source must use HTTPS"
            );
            assert!(
                !entry.source.verified_at.trim().is_empty(),
                "curated pricing source verification date is required"
            );
        }
        overlay
    })
}

async fn read_catalog_settings(pool: &SqlitePool) -> Result<ModelPricingSettings, String> {
    init_model_pricing_table(pool).await?;
    let row = sqlx::query("SELECT catalog_json FROM model_pricing_catalog_cache WHERE id = ?;")
        .bind(SETTINGS_ROW_ID)
        .fetch_optional(pool)
        .await
        .map_err(|err| format!("Failed to read cached model pricing catalog: {err}"))?;
    let Some(row) = row else {
        return Ok(default_model_pricing_settings());
    };
    let json = row
        .try_get::<String, _>("catalog_json")
        .map_err(|err| format!("Failed to decode cached model pricing catalog: {err}"))?;
    match parse_catalog(&json) {
        Ok(settings) => Ok(settings),
        Err(err) => {
            tracing::warn!(error = %err, "cached model pricing catalog invalid; using bundled fallback");
            Ok(default_model_pricing_settings())
        }
    }
}

async fn read_effective_settings(
    pool: &SqlitePool,
    catalog: &ModelPricingSettings,
) -> Result<ModelPricingSettings, String> {
    let row = sqlx::query("SELECT overrides_json FROM model_pricing_overrides WHERE id = ?;")
        .bind(SETTINGS_ROW_ID)
        .fetch_optional(pool)
        .await
        .map_err(|err| format!("Failed to read model pricing overrides: {err}"))?;
    let overrides = match row {
        Some(row) => {
            let json = row
                .try_get::<String, _>("overrides_json")
                .map_err(|err| format!("Failed to decode model pricing overrides: {err}"))?;
            serde_json::from_str::<ModelPricingOverrides>(&json)
                .map_err(|err| format!("Failed to parse model pricing overrides: {err}"))?
        }
        None => ModelPricingOverrides::default(),
    };
    effective_settings(catalog, &overrides)
}

fn effective_settings(
    catalog: &ModelPricingSettings,
    overrides: &ModelPricingOverrides,
) -> Result<ModelPricingSettings, String> {
    let deleted: HashSet<String> = overrides
        .deleted_model_ids
        .iter()
        .map(|model_id| normalize_model_alias(model_id))
        .collect();
    let normalized_upserts = if overrides.upserts.is_empty() {
        Vec::new()
    } else {
        normalize_models(overrides.upserts.clone())?
    };
    let mut upserts: HashMap<String, ModelPricingModel> = normalized_upserts
        .into_iter()
        .map(|model| (normalize_model_alias(&model.model_id), model))
        .collect();
    let mut models = Vec::with_capacity(catalog.models.len() + upserts.len());
    for model in &catalog.models {
        let key = normalize_model_alias(&model.model_id);
        if deleted.contains(&key) {
            continue;
        }
        models.push(upserts.remove(&key).unwrap_or_else(|| model.clone()));
    }
    models.extend(upserts.into_values());
    let models = normalize_models(models)?;
    let version = if overrides == &ModelPricingOverrides::default() {
        catalog.version.clone()
    } else {
        pricing_version_for_models(&models)?
    };
    Ok(ModelPricingSettings {
        version,
        source: catalog.source.clone(),
        models,
    })
}

fn diff_overrides(
    catalog: &[ModelPricingModel],
    effective: &[ModelPricingModel],
) -> ModelPricingOverrides {
    let catalog_by_id: HashMap<String, &ModelPricingModel> = catalog
        .iter()
        .map(|model| (normalize_model_alias(&model.model_id), model))
        .collect();
    let effective_ids: HashSet<String> = effective
        .iter()
        .map(|model| normalize_model_alias(&model.model_id))
        .collect();
    let upserts = effective
        .iter()
        .filter(|model| {
            catalog_by_id
                .get(&normalize_model_alias(&model.model_id))
                .is_none_or(|catalog_model| *catalog_model != *model)
        })
        .cloned()
        .collect();
    let deleted_model_ids = catalog
        .iter()
        .filter(|model| !effective_ids.contains(&normalize_model_alias(&model.model_id)))
        .map(|model| model.model_id.clone())
        .collect();
    ModelPricingOverrides {
        upserts,
        deleted_model_ids,
    }
}

fn normalize_models(models: Vec<ModelPricingModel>) -> Result<Vec<ModelPricingModel>, String> {
    if models.is_empty() {
        return Err("At least one model price is required.".to_string());
    }
    let mut lookup_keys = HashSet::new();
    let mut normalized = Vec::with_capacity(models.len());
    for mut model in models {
        model.model_id = model.model_id.trim().to_string();
        if model.model_id.is_empty() {
            return Err("Model id is required.".to_string());
        }
        if model.price_multiplier_scaled == 0 {
            return Err(format!(
                "Price multiplier must be positive for model {}",
                model.model_id
            ));
        }
        if let Some(long) = model.long_context.as_ref() {
            if long.threshold == 0
                || long.input_multiplier_scaled == 0
                || long.output_multiplier_scaled == 0
            {
                return Err(format!(
                    "Long context threshold and multipliers must be positive for model {}",
                    model.model_id
                ));
            }
        }
        let mut aliases = Vec::new();
        let mut row_keys: HashSet<String> =
            model_lookup_keys(&model.model_id).into_iter().collect();
        for alias in model.aliases {
            let alias = alias.trim();
            if alias.is_empty() {
                continue;
            }
            let alias_keys = model_lookup_keys(alias);
            if alias_keys.iter().all(|key| row_keys.contains(key)) {
                continue;
            }
            row_keys.extend(alias_keys);
            aliases.push(alias.to_string());
        }
        for key in row_keys {
            if !lookup_keys.insert(key.clone()) {
                return Err(format!("Duplicate model pricing alias: {key}"));
            }
        }
        model.aliases = aliases;
        model.service_tier_profiles = model
            .service_tier_profiles
            .into_iter()
            .map(|(tier, profile)| (normalize_service_tier(Some(&tier)), profile))
            .collect();
        normalized.push(model);
    }
    Ok(normalized)
}

async fn backfill_request_log_costs_with_settings(
    pool: &SqlitePool,
    settings: &ModelPricingSettings,
) -> Result<(), String> {
    let _guard = REQUEST_LOG_BACKFILL_LOCK.lock().await;
    backfill_request_log_costs_with_settings_locked(pool, settings).await
}

async fn backfill_request_log_costs_with_settings_locked(
    pool: &SqlitePool,
    settings: &ModelPricingSettings,
) -> Result<(), String> {
    let columns = sqlx::query("PRAGMA table_info(request_logs);")
        .fetch_all(pool)
        .await
        .map_err(|err| format!("Failed to inspect request log pricing columns: {err}"))?;
    let column_names: HashSet<String> = columns
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect();
    if !column_names.contains("uncached_input_tokens") {
        return Ok(());
    }
    let end_id = backfill_mark::max_request_log_id(pool).await?;
    // 同一价格版本已处理过的日志不会再变；版本变化时 read 返回 0，重扫全表。
    let mut window_start = backfill_mark::read(pool, COST_BACKFILL_MARK, &settings.version).await?;
    if window_start >= end_id {
        return Ok(());
    }
    let started_at = Instant::now();
    let mut updated_rows = 0_u64;
    while window_start < end_id {
        let window_end = window_start
            .saturating_add(COST_BACKFILL_WINDOW)
            .min(end_id);
        updated_rows = updated_rows.saturating_add(
            backfill_request_log_cost_window(pool, settings, window_start, window_end).await?,
        );
        window_start = window_end;
    }
    backfill_mark::write(pool, COST_BACKFILL_MARK, &settings.version, end_id).await?;
    if updated_rows > 0 {
        tracing::info!(
            version = %settings.version,
            updated_rows,
            elapsed_ms = started_at.elapsed().as_millis(),
            "request log cost backfill completed"
        );
    }
    Ok(())
}

async fn backfill_request_log_cost_window(
    pool: &SqlitePool,
    settings: &ModelPricingSettings,
    after_id: i64,
    through_id: i64,
) -> Result<u64, String> {
    let rows = sqlx::query(
        r#"
SELECT
  id,
  model,
  mapped_model,
  service_tier,
  uncached_input_tokens,
  cache_read_tokens,
  cache_write_tokens,
  cache_write_5m_tokens,
  cache_write_1h_tokens,
  output_tokens,
  image_input_tokens,
  image_output_tokens
FROM request_logs
WHERE id > ? AND id <= ? AND (
  pricing_version IS NULL OR pricing_version != ?
  -- 旧记录可能已有当前价格版本，但因模型未匹配而没有费用，仍需回算。
  OR (cost_nano_usd IS NULL AND (
    NULLIF(TRIM(mapped_model), '') IS NOT NULL
    OR NULLIF(TRIM(model), '') IS NOT NULL
  ))
);
"#,
    )
    .bind(after_id)
    .bind(through_id)
    .bind(&settings.version)
    .fetch_all(pool)
    .await
    .map_err(|err| format!("Failed to read request log costs for backfill: {err}"))?;
    if rows.is_empty() {
        return Ok(0);
    }
    let updated_rows = rows.len() as u64;
    let mut tx = pool
        .begin()
        .await
        .map_err(|err| format!("Failed to begin request log cost backfill transaction: {err}"))?;
    for row in rows {
        let id = row
            .try_get::<i64, _>("id")
            .map_err(|err| format!("Failed to decode request_logs.id: {err}"))?;
        let usage = BillableUsage {
            uncached_input_tokens: row_u64(&row, "uncached_input_tokens"),
            cache_read_tokens: row_u64(&row, "cache_read_tokens"),
            cache_write_tokens: row_u64(&row, "cache_write_tokens"),
            cache_write_5m_tokens: row_u64(&row, "cache_write_5m_tokens"),
            cache_write_1h_tokens: row_u64(&row, "cache_write_1h_tokens"),
            output_tokens: row_u64(&row, "output_tokens"),
            image_input_tokens: row_u64(&row, "image_input_tokens"),
            image_output_tokens: row_u64(&row, "image_output_tokens"),
        };
        let model = row.try_get::<Option<String>, _>("model").ok().flatten();
        let mapped_model = row
            .try_get::<Option<String>, _>("mapped_model")
            .ok()
            .flatten();
        let service_tier = row
            .try_get::<Option<String>, _>("service_tier")
            .ok()
            .flatten();
        let cost = calculate_request_cost(
            settings,
            model.as_deref(),
            mapped_model.as_deref(),
            service_tier.as_deref(),
            &usage,
        );
        sqlx::query(
            r#"
UPDATE request_logs
SET cost_nano_usd = ?, pricing_version = ?, pricing_model = ?, pricing_context_tier = ?
WHERE id = ?;
"#,
        )
        .bind(cost.as_ref().map(|cost| to_i64(cost.cost_nano_usd)))
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
        .map_err(|err| format!("Failed to commit request log cost backfill: {err}"))?;
    Ok(updated_rows)
}

fn price_component(tokens: u64, price: Option<u64>, multiplier_scaled: u64) -> Option<u64> {
    if tokens == 0 {
        return Some(0);
    }
    let adjusted_price = apply_scaled_multiplier(price?, multiplier_scaled);
    Some(tokens.saturating_mul(adjusted_price))
}

fn apply_breakdown_multiplier(breakdown: &mut RequestCostBreakdown, multiplier_scaled: u64) {
    for value in [
        &mut breakdown.uncached_input_nano_usd,
        &mut breakdown.cache_read_nano_usd,
        &mut breakdown.cache_write_nano_usd,
        &mut breakdown.cache_write_5m_nano_usd,
        &mut breakdown.cache_write_1h_nano_usd,
        &mut breakdown.output_nano_usd,
        &mut breakdown.image_input_nano_usd,
        &mut breakdown.image_output_nano_usd,
    ] {
        *value = apply_scaled_multiplier(*value, multiplier_scaled);
    }
}

fn apply_scaled_multiplier(value: u64, multiplier_scaled: u64) -> u64 {
    let numerator = (value as u128)
        .saturating_mul(multiplier_scaled as u128)
        .saturating_add((PRICE_MULTIPLIER_SCALE / 2) as u128);
    u64::try_from(numerator / PRICE_MULTIPLIER_SCALE as u128).unwrap_or(u64::MAX)
}

fn default_price_multiplier_scaled() -> u64 {
    PRICE_MULTIPLIER_SCALE
}

fn normalize_service_tier(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("standard")
        .to_ascii_lowercase()
}

fn find_model_price<'a>(
    settings: &'a ModelPricingSettings,
    model: &str,
) -> Option<&'a ModelPricingModel> {
    let lookup_keys: HashSet<String> = model_lookup_keys(model).into_iter().collect();
    settings.models.iter().find(|price| {
        model_lookup_keys(&price.model_id)
            .into_iter()
            .any(|key| lookup_keys.contains(&key))
            || price.aliases.iter().any(|alias| {
                model_lookup_keys(alias)
                    .into_iter()
                    .any(|key| lookup_keys.contains(&key))
            })
    })
}

fn model_lookup_keys(model: &str) -> Vec<String> {
    let normalized = normalize_model_alias(model);
    let mut keys = Vec::new();
    push_model_lookup_key(&mut keys, &normalized);
    if let Some((_, suffix)) = normalized.split_once('/') {
        push_model_lookup_key(&mut keys, suffix);
    }
    keys
}

fn push_model_lookup_key(keys: &mut Vec<String>, value: &str) {
    let key = match value {
        "claude-opus-4.8" => "claude-opus-4-8".to_string(),
        "claude-opus-4.7" => "claude-opus-4-7".to_string(),
        // 官方 ID 是连字符 claude-opus-5-5；点号写法对齐 opus-4.8。
        "claude-opus-5.5" => "claude-opus-5-5".to_string(),
        _ => value.to_string(),
    };
    if !key.is_empty() && !keys.contains(&key) {
        keys.push(key);
    }
}

fn normalize_model_alias(model: &str) -> String {
    model
        .trim()
        .to_ascii_lowercase()
        .replace(char::is_whitespace, "-")
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

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn row_u64(row: &sqlx::sqlite::SqliteRow, column: &str) -> u64 {
    row.try_get::<Option<i64>, _>(column)
        .ok()
        .flatten()
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0)
}

fn now_ms_i64() -> i64 {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    fn usage() -> BillableUsage {
        BillableUsage {
            uncached_input_tokens: 100,
            cache_read_tokens: 20,
            cache_write_tokens: 5,
            cache_write_5m_tokens: 3,
            cache_write_1h_tokens: 2,
            output_tokens: 10,
            image_input_tokens: 0,
            image_output_tokens: 0,
        }
    }

    fn custom_model() -> ModelPricingModel {
        ModelPricingModel {
            model_id: "custom-model".to_string(),
            aliases: vec!["openai/custom-model".to_string()],
            price_multiplier_scaled: PRICE_MULTIPLIER_SCALE,
            standard: ModelPricingProfile {
                input_nano_usd_per_token: Some(100),
                output_nano_usd_per_token: Some(200),
                cache_read_nano_usd_per_token: Some(10),
                cache_write_nano_usd_per_token: Some(125),
                cache_write_5m_nano_usd_per_token: Some(125),
                cache_write_1h_nano_usd_per_token: Some(200),
                image_input_nano_usd_per_token: None,
                image_output_nano_usd_per_token: None,
            },
            service_tier_profiles: BTreeMap::new(),
            long_context: Some(LongContextPricing {
                threshold: 100,
                input_multiplier_scaled: PRICE_MULTIPLIER_SCALE * 2,
                output_multiplier_scaled: PRICE_MULTIPLIER_SCALE * 3 / 2,
            }),
        }
    }

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        crate::sqlite::init_schema(&pool)
            .await
            .expect("init sqlite");
        pool
    }

    #[test]
    fn bundled_catalog_uses_latest_upstream_snapshot() {
        let settings = default_model_pricing_settings();
        assert_eq!(
            settings.version,
            "catalog.69854741.b88b66df+curated.20261027"
        );
        let source = settings.source.expect("catalog source");
        assert_eq!(source.commit, "698547418fc8b8fc5f597fd34516e7026e706d82");

        let model = settings
            .models
            .iter()
            .find(|model| model.model_id == "codex-auto-review")
            .expect("codex-auto-review pricing");
        assert_eq!(model.aliases, vec!["openai/codex-auto-review"]);
        assert_eq!(model.standard.input_nano_usd_per_token, Some(200));
        assert_eq!(model.standard.output_nano_usd_per_token, Some(1_200));
        assert_eq!(model.standard.cache_read_nano_usd_per_token, Some(20));
        assert!(model.service_tier_profiles.is_empty());
        assert!(model.long_context.is_none());
    }

    #[test]
    fn curated_overlay_declares_official_model_sources() {
        let overlay = curated_pricing_overlay();
        let sources = overlay
            .models
            .iter()
            .map(|entry| {
                (
                    entry.model.model_id.as_str(),
                    (
                        entry.source.pricing_url.as_str(),
                        entry.source.verified_at.as_str(),
                    ),
                )
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(
            sources.get("grok-4.7"),
            Some(&("https://docs.x.ai/developers/pricing", "2026-09-22"))
        );
        assert_eq!(
            sources.get("mimo-v2.6-pro"),
            Some(&(
                "https://mimo.mi.com/docs/en-US/price/pay-as-you-go",
                "2026-09-22"
            ))
        );
        assert_eq!(
            sources.get("mimo-v2.6-flash"),
            Some(&(
                "https://mimo.mi.com/docs/en-US/price/pay-as-you-go",
                "2026-09-22"
            ))
        );
        assert_eq!(
            sources.get("grok-4.6"),
            Some(&("https://docs.x.ai/developers/pricing", "2026-08-13"))
        );
        assert_eq!(
            sources.get("kimi-k3"),
            Some(&(
                "https://platform.kimi.ai/docs/pricing/chat-k3",
                "2026-07-29"
            ))
        );
        assert_eq!(
            sources.get("gemini-3.5-flash-lite"),
            Some(&(
                "https://ai.google.dev/gemini-api/docs/pricing",
                "2026-07-29"
            ))
        );
        assert_eq!(
            sources.get("gemini-2.5-pro"),
            Some(&(
                "https://ai.google.dev/gemini-api/docs/pricing",
                "2026-09-01"
            ))
        );
        assert_eq!(
            sources.get("claude-fable-5-1"),
            Some(&(
                "https://platform.claude.com/docs/en/about-claude/pricing",
                "2026-09-04"
            ))
        );
        assert_eq!(
            sources.get("gpt-6-astra"),
            Some(&(
                "https://developers.openai.com/api/docs/pricing",
                "2026-10-03"
            ))
        );
        assert_eq!(
            sources.get("gpt-6-sol"),
            Some(&(
                "https://developers.openai.com/api/docs/models/gpt-6-sol",
                "2026-09-23"
            ))
        );
        assert_eq!(
            sources.get("gpt-6.1-sol"),
            Some(&(
                "https://developers.openai.com/api/docs/models/gpt-6.1-sol",
                "2026-09-29"
            ))
        );
        assert_eq!(
            sources.get("gpt-reserve"),
            Some(&(
                "https://developers.openai.com/api/docs/pricing",
                "2026-09-30"
            ))
        );
        assert_eq!(
            sources.get("gpt-6-luna"),
            Some(&(
                "https://developers.openai.com/api/docs/models/gpt-6-luna",
                "2026-09-23"
            ))
        );
        assert_eq!(
            sources.get("claude-opus-5-5"),
            Some(&(
                "https://platform.claude.com/docs/en/about-claude/pricing",
                "2026-09-23"
            ))
        );
        assert_eq!(
            sources.get("claude-sonnet-5-5"),
            Some(&(
                "https://platform.claude.com/docs/en/about-claude/pricing",
                "2026-10-07"
            ))
        );
        for model in ["claude-haiku-5-5", "claude-mythos-5-1", "claude-mythos-5"] {
            assert_eq!(
                sources.get(model),
                Some(&(
                    "https://platform.claude.com/docs/en/about-claude/pricing",
                    "2026-10-07"
                )),
                "{model}"
            );
        }
    }

    #[test]
    fn gemini_3_6_flash_aliases_apply_standard_and_priority_prices() {
        let settings = default_model_pricing_settings();
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        for model in [
            "gemini-3.6-flash",
            "google/gemini-3.6-flash",
            "models/gemini-3.6-flash",
        ] {
            let standard = calculate_request_cost(&settings, Some(model), None, None, &usage)
                .expect("Gemini 3.6 Flash standard price");
            let priority =
                calculate_request_cost(&settings, Some(model), None, Some("priority"), &usage)
                    .expect("Gemini 3.6 Flash priority price");

            assert_eq!(standard.pricing_model, "gemini-3.6-flash");
            assert_eq!(standard.cost_nano_usd, 9_150);
            assert_eq!(priority.cost_nano_usd, 16_470);
        }
    }

    #[test]
    fn grok_4_7_aliases_apply_official_short_and_long_context_prices() {
        let settings = default_model_pricing_settings();
        let short_usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        let long_usage = BillableUsage {
            uncached_input_tokens: 200_001,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        // Official xAI grok-4.7: $2 / $0.50 / $6 per 1M; 200k+ prompts bill 2x.
        // https://docs.x.ai/developers/pricing verified 2026-09-22.
        for model in [
            "grok-4.7",
            "grok-4.7-high",
            "grok-4.7-xhigh",
            "grok-4.7xhigh",
            "grok4.7xhigh",
            "x-ai/grok-4.7",
            "xai/grok-4.7-high",
            "x-ai/grok-4.7-xhigh",
        ] {
            let short = calculate_request_cost(&settings, Some(model), None, None, &short_usage)
                .expect("Grok 4.7 short-context price");
            let long = calculate_request_cost(&settings, Some(model), None, None, &long_usage)
                .expect("Grok 4.7 long-context price");

            assert_eq!(short.pricing_model, "grok-4.7");
            assert_eq!(short.cost_nano_usd, 8_500);
            assert_eq!(short.context_tier, PricingContextTier::Standard);
            assert_eq!(long.pricing_model, "grok-4.7");
            assert_eq!(long.cost_nano_usd, 800_016_000);
            assert_eq!(long.context_tier, PricingContextTier::Long);
        }
        assert!(settings.version.contains("+curated.20261027"));
    }

    #[test]
    fn mimo_v2_6_aliases_apply_official_realtime_prices() {
        let settings = default_model_pricing_settings();
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        // Overseas realtime USD, https://mimo.mi.com/docs/en-US/price/pay-as-you-go (2026-09-22).
        // Cache write is free. Cache hit $0.0036 / $0.0028 per 1M is 3.6 / 2.8 nano-USD
        // per token; the catalog unit is integer nano-USD, so those rates round to 4 and 3.
        for model in ["mimo-v2.6-pro", "xiaomi/mimo-v2.6-pro"] {
            let cost = calculate_request_cost(&settings, Some(model), None, None, &usage)
                .expect("MiMo V2.6 Pro price");
            assert_eq!(cost.pricing_model, "mimo-v2.6-pro");
            assert_eq!(cost.breakdown.uncached_input_nano_usd, 435);
            assert_eq!(cost.breakdown.cache_read_nano_usd, 4);
            assert_eq!(cost.breakdown.cache_write_nano_usd, 0);
            assert_eq!(cost.breakdown.output_nano_usd, 870);
            assert_eq!(cost.cost_nano_usd, 1_309);
            assert_eq!(cost.context_tier, PricingContextTier::Standard);
        }

        for model in ["mimo-v2.6-flash", "xiaomi/mimo-v2.6-flash"] {
            let cost = calculate_request_cost(&settings, Some(model), None, None, &usage)
                .expect("MiMo V2.6 Flash price");
            assert_eq!(cost.pricing_model, "mimo-v2.6-flash");
            assert_eq!(cost.breakdown.uncached_input_nano_usd, 140);
            assert_eq!(cost.breakdown.cache_read_nano_usd, 3);
            assert_eq!(cost.breakdown.cache_write_nano_usd, 0);
            assert_eq!(cost.breakdown.output_nano_usd, 280);
            assert_eq!(cost.cost_nano_usd, 423);
        }
    }

    #[test]
    fn grok_4_6_aliases_apply_official_short_and_long_context_prices() {
        let settings = default_model_pricing_settings();
        let short_usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        let long_usage = BillableUsage {
            uncached_input_tokens: 200_001,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        // Official xAI grok-4.6: $2 / $0.50 / $6 per 1M; 200k+ prompts bill 2x.
        for model in [
            "grok-4.6",
            "grok-4.6-high",
            "grok-4.6-xhigh",
            "grok-4.6xhigh",
            "grok4.6xhigh",
            "x-ai/grok-4.6",
            "xai/grok-4.6-high",
            "x-ai/grok-4.6-xhigh",
        ] {
            let short = calculate_request_cost(&settings, Some(model), None, None, &short_usage)
                .expect("Grok 4.6 short-context price");
            let long = calculate_request_cost(&settings, Some(model), None, None, &long_usage)
                .expect("Grok 4.6 long-context price");

            assert_eq!(short.pricing_model, "grok-4.6");
            assert_eq!(short.cost_nano_usd, 8_500);
            assert_eq!(short.context_tier, PricingContextTier::Standard);
            assert_eq!(long.pricing_model, "grok-4.6");
            assert_eq!(long.cost_nano_usd, 800_016_000);
            assert_eq!(long.context_tier, PricingContextTier::Long);
        }
        assert!(settings.version.contains("+curated.20261027"));
    }

    #[test]
    fn claude_fable_5_1_aliases_apply_official_cache_read_discount() {
        let settings = default_model_pricing_settings();
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_5m_tokens: 1,
            cache_write_1h_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        // Official Fable 5.1: $10 / $0.25 cache read / $12.50 5m / $20 1h / $50 per 1M.
        for model in [
            "claude-fable-5-1",
            "anthropic/claude-fable-5-1",
            "claude-fable-5.1",
            "anthropic/claude-fable-5.1",
        ] {
            let cost = calculate_request_cost(&settings, Some(model), None, None, &usage)
                .expect("Fable 5.1 official price");
            assert_eq!(cost.pricing_model, "claude-fable-5-1", "{model}");
            assert_eq!(cost.cost_nano_usd, 92_750, "{model}");
            assert_eq!(cost.breakdown.cache_read_nano_usd, 250, "{model}");
            assert_eq!(cost.context_tier, PricingContextTier::Standard, "{model}");
        }

        let fable_5 = calculate_request_cost(&settings, Some("claude-fable-5"), None, None, &usage)
            .expect("Fable 5 baseline price");
        assert_eq!(fable_5.pricing_model, "claude-fable-5");
        assert_eq!(fable_5.breakdown.cache_read_nano_usd, 1_000);
        assert_ne!(fable_5.cost_nano_usd, 92_750);
    }

    #[test]
    fn gpt_6_astra_aliases_apply_official_tiers_and_long_context() {
        let settings = default_model_pricing_settings();
        let short_usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        let long_usage = BillableUsage {
            uncached_input_tokens: 272_001,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        // Official GPT-6 Astra: $10 / $1 cache / $12.50 write / $50; 272k+ bills 2x input and 1.5x output.
        for model in ["gpt-6-astra", "openai/gpt-6-astra"] {
            let standard = calculate_request_cost(&settings, Some(model), None, None, &short_usage)
                .expect("GPT-6 Astra standard price");
            let priority = calculate_request_cost(
                &settings,
                Some(model),
                None,
                Some("priority"),
                &short_usage,
            )
            .expect("GPT-6 Astra priority price");
            let fast =
                calculate_request_cost(&settings, Some(model), None, Some("fast"), &short_usage)
                    .expect("GPT-6 Astra fast price");
            let flex =
                calculate_request_cost(&settings, Some(model), None, Some("flex"), &short_usage)
                    .expect("GPT-6 Astra flex price");
            // Ultrafast 仅 Astra 公开，官方总表为 Standard 的 6x（2026-10-03 核对）。
            let ultrafast = calculate_request_cost(
                &settings,
                Some(model),
                None,
                Some("ultrafast"),
                &short_usage,
            )
            .expect("GPT-6 Astra ultrafast price");
            let long_ultrafast = calculate_request_cost(
                &settings,
                Some(model),
                None,
                Some("ultrafast"),
                &long_usage,
            )
            .expect("GPT-6 Astra long ultrafast price");
            let long = calculate_request_cost(&settings, Some(model), None, None, &long_usage)
                .expect("GPT-6 Astra long-context price");

            assert_eq!(standard.pricing_model, "gpt-6-astra", "{model}");
            assert_eq!(standard.cost_nano_usd, 73_500, "{model}");
            assert_eq!(priority.cost_nano_usd, 147_000, "{model}");
            assert_eq!(fast.cost_nano_usd, 147_000, "{model}");
            assert_eq!(flex.cost_nano_usd, 36_750, "{model}");
            assert_eq!(ultrafast.cost_nano_usd, 441_000, "{model}");
            assert_eq!(ultrafast.service_tier, "ultrafast", "{model}");
            assert_eq!(long.cost_nano_usd, 5_440_095_000, "{model}");
            assert_eq!(long_ultrafast.cost_nano_usd, 32_640_570_000, "{model}");
            assert_eq!(long.context_tier, PricingContextTier::Long, "{model}");
        }
        assert!(settings.version.contains("+curated.20261027"));
    }

    #[test]
    fn gpt_6_sol_and_luna_apply_official_tiers_and_long_context() {
        let settings = default_model_pricing_settings();
        let short_usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        let long_usage = BillableUsage {
            uncached_input_tokens: 272_001,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        // Official GPT-6 Sol: $2 / $0.20 cache / $2.50 write / $10; >272K is 2x input and cache, 1.5x output.
        // https://developers.openai.com/api/docs/models/gpt-6-sol verified 2026-09-23.
        for model in ["gpt-6-sol", "openai/gpt-6-sol"] {
            let standard = calculate_request_cost(&settings, Some(model), None, None, &short_usage)
                .expect("GPT-6 Sol standard price");
            let fast =
                calculate_request_cost(&settings, Some(model), None, Some("fast"), &short_usage)
                    .expect("GPT-6 Sol fast price");
            let flex =
                calculate_request_cost(&settings, Some(model), None, Some("flex"), &short_usage)
                    .expect("GPT-6 Sol flex price");
            let long = calculate_request_cost(&settings, Some(model), None, None, &long_usage)
                .expect("GPT-6 Sol long-context price");

            assert_eq!(standard.pricing_model, "gpt-6-sol", "{model}");
            assert_eq!(standard.cost_nano_usd, 14_700, "{model}");
            assert_eq!(fast.cost_nano_usd, 29_400, "{model}");
            assert_eq!(flex.cost_nano_usd, 7_350, "{model}");
            assert_eq!(long.cost_nano_usd, 1_088_019_000, "{model}");
            assert_eq!(long.context_tier, PricingContextTier::Long, "{model}");
        }

        // Official GPT-6.1 Sol only halves cache read vs GPT-6 Sol: $2 / $0.10 cache / $2.50 write / $10.
        // https://developers.openai.com/api/docs/models/gpt-6.1-sol verified 2026-09-29.
        for model in ["gpt-6.1-sol", "openai/gpt-6.1-sol"] {
            let standard = calculate_request_cost(&settings, Some(model), None, None, &short_usage)
                .expect("GPT-6.1 Sol standard price");
            let fast =
                calculate_request_cost(&settings, Some(model), None, Some("fast"), &short_usage)
                    .expect("GPT-6.1 Sol fast price");
            let flex =
                calculate_request_cost(&settings, Some(model), None, Some("flex"), &short_usage)
                    .expect("GPT-6.1 Sol flex price");
            let long = calculate_request_cost(&settings, Some(model), None, None, &long_usage)
                .expect("GPT-6.1 Sol long-context price");

            assert_eq!(standard.pricing_model, "gpt-6.1-sol", "{model}");
            assert_eq!(standard.breakdown.cache_read_nano_usd, 100, "{model}");
            assert_eq!(standard.cost_nano_usd, 14_600, "{model}");
            assert_eq!(fast.cost_nano_usd, 29_200, "{model}");
            assert_eq!(flex.cost_nano_usd, 7_300, "{model}");
            assert_eq!(long.cost_nano_usd, 1_088_019_000, "{model}");
            assert_eq!(long.context_tier, PricingContextTier::Long, "{model}");
        }

        // Official GPT-6 Luna: $0.10 / $0.01 / $0.125 / $0.50. Flex cache write is $0.0625/MTok
        // = 62.5 nano-USD/token; the catalog unit is integer nano-USD, so that rate rounds to 63.
        for model in ["gpt-6-luna", "openai/gpt-6-luna"] {
            let standard = calculate_request_cost(&settings, Some(model), None, None, &short_usage)
                .expect("GPT-6 Luna standard price");
            let flex =
                calculate_request_cost(&settings, Some(model), None, Some("flex"), &short_usage)
                    .expect("GPT-6 Luna flex price");
            let long = calculate_request_cost(&settings, Some(model), None, None, &long_usage)
                .expect("GPT-6 Luna long-context price");

            assert_eq!(standard.pricing_model, "gpt-6-luna", "{model}");
            assert_eq!(standard.cost_nano_usd, 735, "{model}");
            assert_eq!(flex.breakdown.cache_write_nano_usd, 63, "{model}");
            assert_eq!(flex.cost_nano_usd, 368, "{model}");
            assert_eq!(long.cost_nano_usd, 54_400_950, "{model}");
        }
    }

    #[test]
    fn gpt61_terra_prices_cover_aliases_tiers_and_long_context_boundary() {
        let settings = default_model_pricing_settings();
        // 官方 2026-10-27：$1 输入、$0.05 缓存读、$1.25 默认缓存写、$5 输出 / 1M。
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        for model in ["gpt-6.1-terra", "openai/gpt-6.1-terra"] {
            for (tier, expected) in [
                (None, 7_300),
                (Some("fast"), 14_600),
                (Some("priority"), 14_600),
                (Some("flex"), 3_650),
                (Some("batch"), 3_650),
            ] {
                let cost =
                    calculate_request_cost(&settings, Some(model), None, tier, &usage).unwrap();
                assert_eq!(cost.pricing_model, "gpt-6.1-terra");
                assert_eq!(cost.cost_nano_usd, expected);
            }
            let at_boundary = BillableUsage {
                uncached_input_tokens: 271_998,
                ..usage.clone()
            };
            let at =
                calculate_request_cost(&settings, Some(model), None, None, &at_boundary).unwrap();
            assert_eq!(at.context_tier, PricingContextTier::Standard);
            let above = BillableUsage {
                uncached_input_tokens: 271_999,
                ..usage.clone()
            };
            let long = calculate_request_cost(&settings, Some(model), None, None, &above).unwrap();
            assert_eq!(long.context_tier, PricingContextTier::Long);
            assert_eq!(long.breakdown.cache_read_nano_usd, 100);
            assert_eq!(long.breakdown.cache_write_nano_usd, 2_500);
            assert_eq!(long.cost_nano_usd, 544_008_100);
        }
    }

    #[test]
    fn claude_opus_5_5_aliases_apply_official_prices() {
        let settings = default_model_pricing_settings();
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_5m_tokens: 1,
            cache_write_1h_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        // Official Opus 5.5: $4 / $0.20 cache read / $5 5m / $8 1h / $20 per 1M.
        // https://platform.claude.com/docs/en/about-claude/pricing verified 2026-09-23.
        for model in [
            "claude-opus-5-5",
            "anthropic/claude-opus-5-5",
            "claude-opus-5.5",
            "anthropic/claude-opus-5.5",
        ] {
            let cost = calculate_request_cost(&settings, Some(model), None, None, &usage)
                .expect("Opus 5.5 official price");
            assert_eq!(cost.pricing_model, "claude-opus-5-5", "{model}");
            assert_eq!(cost.breakdown.cache_read_nano_usd, 200, "{model}");
            assert_eq!(cost.cost_nano_usd, 37_200, "{model}");
        }
    }

    #[test]
    fn claude_sonnet_5_5_aliases_apply_official_prices() {
        let settings = default_model_pricing_settings();

        // Official Sonnet 5.5 prices apply across the full 1M context window.
        // https://platform.claude.com/docs/en/about-claude/pricing verified 2026-10-07.
        for model in [
            "claude-sonnet-5-5",
            "anthropic/claude-sonnet-5-5",
            "claude-sonnet-5.5",
            "anthropic/claude-sonnet-5.5",
        ] {
            for input_tokens in [1, 200_001] {
                let usage = BillableUsage {
                    uncached_input_tokens: input_tokens,
                    cache_read_tokens: 1,
                    cache_write_tokens: 1,
                    cache_write_5m_tokens: 1,
                    cache_write_1h_tokens: 1,
                    output_tokens: 1,
                    ..BillableUsage::default()
                };
                let cost = calculate_request_cost(&settings, Some(model), None, None, &usage)
                    .expect("Sonnet 5.5 official price");
                assert_eq!(cost.pricing_model, "claude-sonnet-5-5", "{model}");
                assert_eq!(cost.context_tier, PricingContextTier::Standard, "{model}");
                assert_eq!(
                    cost.breakdown.uncached_input_nano_usd,
                    input_tokens * 2_000,
                    "{model}"
                );
                assert_eq!(cost.breakdown.cache_read_nano_usd, 100, "{model}");
                assert_eq!(cost.breakdown.cache_write_nano_usd, 2_500, "{model}");
                assert_eq!(cost.breakdown.cache_write_5m_nano_usd, 2_500, "{model}");
                assert_eq!(cost.breakdown.cache_write_1h_nano_usd, 4_000, "{model}");
                assert_eq!(cost.breakdown.output_nano_usd, 10_000, "{model}");
                assert_eq!(cost.cost_nano_usd, input_tokens * 2_000 + 19_100, "{model}");
            }
        }
    }

    #[test]
    fn claude_haiku_5_5_aliases_apply_prompt_length_prices() {
        let settings = default_model_pricing_settings();

        // 官方按总输入（含缓存）是否超过 100000 token 对整次请求升档；输出不参与阈值。
        // https://platform.claude.com/docs/en/about-claude/pricing 核对于 2026-10-07。
        for model in [
            "claude-haiku-5-5",
            "anthropic/claude-haiku-5-5",
            "claude-haiku-5.5",
            "anthropic/claude-haiku-5.5",
        ] {
            for (input_tokens, multiplier, tier) in [
                (1, 1, PricingContextTier::Standard),
                (99_996, 1, PricingContextTier::Standard),
                (99_997, 5, PricingContextTier::Long),
                (200_001, 5, PricingContextTier::Long),
            ] {
                let usage = BillableUsage {
                    uncached_input_tokens: input_tokens,
                    cache_read_tokens: 1,
                    cache_write_tokens: 1,
                    cache_write_5m_tokens: 1,
                    cache_write_1h_tokens: 1,
                    output_tokens: 10,
                    ..BillableUsage::default()
                };
                let cost = calculate_request_cost(&settings, Some(model), None, None, &usage)
                    .expect("Haiku 5.5 official price");
                assert_eq!(cost.pricing_model, "claude-haiku-5-5", "{model}");
                assert_eq!(cost.context_tier, tier, "{model}, {input_tokens}");
                assert_eq!(
                    cost.breakdown.uncached_input_nano_usd,
                    input_tokens * 100 * multiplier
                );
                assert_eq!(cost.breakdown.cache_read_nano_usd, 10 * multiplier);
                assert_eq!(cost.breakdown.cache_write_nano_usd, 125 * multiplier);
                assert_eq!(cost.breakdown.cache_write_5m_nano_usd, 125 * multiplier);
                assert_eq!(cost.breakdown.cache_write_1h_nano_usd, 200 * multiplier);
                assert_eq!(cost.breakdown.output_nano_usd, 5_000 * multiplier);
                assert_eq!(
                    cost.cost_nano_usd,
                    (input_tokens * 100 + 5_460) * multiplier
                );
            }
        }
    }

    #[test]
    fn claude_mythos_aliases_apply_official_prices() {
        let settings = default_model_pricing_settings();
        let usage = BillableUsage {
            uncached_input_tokens: 200_001,
            cache_read_tokens: 1,
            cache_write_tokens: 1,
            cache_write_5m_tokens: 1,
            cache_write_1h_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        for (model, canonical, cache_read) in [
            ("claude-mythos-5", "claude-mythos-5", 1_000),
            ("anthropic/claude-mythos-5", "claude-mythos-5", 1_000),
            ("claude-mythos-5-1", "claude-mythos-5-1", 250),
            ("anthropic/claude-mythos-5-1", "claude-mythos-5-1", 250),
            ("claude-mythos-5.1", "claude-mythos-5-1", 250),
            ("anthropic/claude-mythos-5.1", "claude-mythos-5-1", 250),
        ] {
            let cost = calculate_request_cost(&settings, Some(model), None, None, &usage)
                .expect("Mythos official price");
            assert_eq!(cost.pricing_model, canonical, "{model}");
            assert_eq!(cost.context_tier, PricingContextTier::Standard, "{model}");
            assert_eq!(cost.breakdown.uncached_input_nano_usd, 2_000_010_000);
            assert_eq!(cost.breakdown.cache_read_nano_usd, cache_read);
            assert_eq!(cost.breakdown.cache_write_nano_usd, 12_500);
            assert_eq!(cost.breakdown.cache_write_5m_nano_usd, 12_500);
            assert_eq!(cost.breakdown.cache_write_1h_nano_usd, 20_000);
            assert_eq!(cost.breakdown.output_nano_usd, 50_000);
            assert_eq!(cost.cost_nano_usd, 2_000_105_000 + cache_read);
        }
    }

    #[test]
    fn curated_models_apply_official_standard_and_priority_prices() {
        let settings = default_model_pricing_settings();
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        for model in ["kimi-k3", "moonshotai/kimi-k3"] {
            let cost = calculate_request_cost(&settings, Some(model), None, None, &usage)
                .expect("Kimi K3 standard price");
            assert_eq!(cost.pricing_model, "kimi-k3");
            assert_eq!(cost.cost_nano_usd, 18_300);
        }

        for model in [
            "gemini-3.5-flash-lite",
            "google/gemini-3.5-flash-lite",
            "models/gemini-3.5-flash-lite",
        ] {
            let standard = calculate_request_cost(&settings, Some(model), None, None, &usage)
                .expect("Gemini 3.5 Flash-Lite standard price");
            let priority =
                calculate_request_cost(&settings, Some(model), None, Some("priority"), &usage)
                    .expect("Gemini 3.5 Flash-Lite priority price");

            assert_eq!(standard.pricing_model, "gemini-3.5-flash-lite");
            assert_eq!(standard.cost_nano_usd, 2_830);
            assert_eq!(priority.cost_nano_usd, 5_090);
        }
        assert!(settings.version.contains("+curated."));
    }

    #[test]
    fn gemini_pro_prices_cover_standard_priority_and_long_context_cache_writes() {
        let settings = default_model_pricing_settings();
        let standard_usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        let long_usage = BillableUsage {
            uncached_input_tokens: 200_001,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        let gemini_25 = calculate_request_cost(
            &settings,
            Some("google/gemini-2.5-pro"),
            None,
            None,
            &standard_usage,
        )
        .expect("Gemini 2.5 Pro standard price");
        assert_eq!(gemini_25.pricing_model, "gemini-2.5-pro");
        assert_eq!(gemini_25.cost_nano_usd, 12_625);
        assert_eq!(gemini_25.breakdown.cache_write_nano_usd, 1_250);

        for model in [
            "gemini-3-pro-preview",
            "gemini-3.1-pro-high",
            "gemini-3.1-pro-low",
            "models/gemini-3.1-pro-preview",
        ] {
            let standard =
                calculate_request_cost(&settings, Some(model), None, None, &standard_usage)
                    .expect("Gemini Pro standard price");
            let priority = calculate_request_cost(
                &settings,
                Some(model),
                None,
                Some("priority"),
                &standard_usage,
            )
            .expect("Gemini Pro priority price");
            let long = calculate_request_cost(&settings, Some(model), None, None, &long_usage)
                .expect("Gemini Pro long-context price");

            assert_eq!(standard.cost_nano_usd, 16_200, "{model}");
            assert_eq!(priority.cost_nano_usd, 29_160, "{model}");
            assert_eq!(long.cost_nano_usd, 800_022_000, "{model}");
            assert_eq!(long.context_tier, PricingContextTier::Long, "{model}");
        }

        let customtools = calculate_request_cost(
            &settings,
            Some("gemini-3.1-pro-preview-customtools"),
            None,
            Some("priority"),
            &standard_usage,
        )
        .expect("Gemini customtools fallback price");
        assert_eq!(customtools.cost_nano_usd, 16_200);
    }

    #[test]
    fn curated_overlay_replaces_base_price_before_user_overrides() {
        let mut catalog = serde_json::from_str::<serde_json::Value>(BUNDLED_PRICING_CATALOG)
            .expect("bundled catalog json");
        catalog["version"] = serde_json::Value::String("remote.with-k3".to_string());
        catalog["models"]
            .as_array_mut()
            .expect("catalog models")
            .push(serde_json::json!({
                "modelId": "kimi-k3",
                "aliases": ["moonshotai/kimi-k3"],
                "standard": {
                    "inputNanoUsdPerToken": 1,
                    "outputNanoUsdPerToken": 1,
                    "cacheReadNanoUsdPerToken": 1
                },
                "serviceTierProfiles": {},
                "longContext": null
            }));
        let base = parse_catalog(&serde_json::to_string(&catalog).expect("catalog body"))
            .expect("catalog with curated overlay");
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        assert_eq!(
            calculate_request_cost(&base, Some("kimi-k3"), None, None, &usage)
                .expect("curated Kimi K3 price")
                .cost_nano_usd,
            18_300
        );

        let mut customized = base.models.clone();
        customized
            .iter_mut()
            .find(|model| model.model_id == "kimi-k3")
            .expect("Kimi K3 model")
            .standard
            .input_nano_usd_per_token = Some(9_999);
        let overrides = diff_overrides(&base.models, &customized);
        let effective = effective_settings(&base, &overrides).expect("effective settings");
        assert_eq!(
            calculate_request_cost(&effective, Some("kimi-k3"), None, None, &usage)
                .expect("custom Kimi K3 price")
                .breakdown
                .uncached_input_nano_usd,
            9_999
        );
    }

    #[test]
    fn claude_opus_5_aliases_use_current_standard_and_cache_prices() {
        let settings = default_model_pricing_settings();
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_5m_tokens: 1,
            cache_write_1h_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };

        for model in ["claude-opus-5", "anthropic/claude-opus-5"] {
            let cost = calculate_request_cost(&settings, Some(model), None, None, &usage)
                .expect("claude opus 5 price");

            assert_eq!(cost.pricing_model, "claude-opus-5");
            assert_eq!(cost.breakdown.uncached_input_nano_usd, 5_000);
            assert_eq!(cost.breakdown.cache_read_nano_usd, 500);
            assert_eq!(cost.breakdown.cache_write_5m_nano_usd, 6_250);
            assert_eq!(cost.breakdown.cache_write_1h_nano_usd, 10_000);
            assert_eq!(cost.breakdown.output_nano_usd, 25_000);
            assert_eq!(cost.cost_nano_usd, 46_750);
        }
    }

    #[test]
    fn prices_all_components_without_double_counting() {
        let settings = ModelPricingSettings {
            version: "test".to_string(),
            source: None,
            models: vec![custom_model()],
        };
        let cost =
            calculate_request_cost(&settings, Some("openai/custom-model"), None, None, &usage())
                .expect("cost");

        assert_eq!(cost.breakdown.uncached_input_nano_usd, 20_000);
        assert_eq!(cost.breakdown.cache_read_nano_usd, 400);
        assert_eq!(cost.breakdown.cache_write_nano_usd, 1_250);
        assert_eq!(cost.breakdown.cache_write_5m_nano_usd, 750);
        assert_eq!(cost.breakdown.cache_write_1h_nano_usd, 800);
        assert_eq!(cost.breakdown.output_nano_usd, 3_000);
        assert_eq!(cost.cost_nano_usd, 26_200);
        assert_eq!(cost.context_tier, PricingContextTier::Long);
    }

    #[test]
    fn gpt_5_6_profiles_apply_standard_priority_and_flex_prices() {
        let settings = default_model_pricing_settings();
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        for (model, expected) in [
            ("gpt-5.6-terra", [16_700, 33_400, 8_350]),
            ("gpt-5.6-luna", [1_670, 3_340, 835]),
            // gpt-reserve 是 Codex 的 Luna Reserve 额度桶，无独立官方价，按 GPT-5.6 Luna 计价。
            ("gpt-reserve", [1_670, 3_340, 835]),
            ("openai/gpt-reserve", [1_670, 3_340, 835]),
        ] {
            for (service_tier, expected_cost) in [
                (None, expected[0]),
                (Some("priority"), expected[1]),
                (Some("fast"), expected[1]),
                (Some("flex"), expected[2]),
            ] {
                let cost =
                    calculate_request_cost(&settings, Some(model), None, service_tier, &usage)
                        .expect("profile cost");

                assert_eq!(
                    cost.cost_nano_usd, expected_cost,
                    "{model} {service_tier:?}"
                );
                assert_eq!(
                    cost.breakdown.cache_write_nano_usd,
                    cost.breakdown.uncached_input_nano_usd * 5 / 4,
                    "{model} {service_tier:?} cache write"
                );
            }
        }
    }

    #[test]
    fn unmatched_models_use_fixed_default_prices() {
        let settings = default_model_pricing_settings();
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            cache_read_tokens: 1,
            cache_write_tokens: 1,
            cache_write_5m_tokens: 1,
            cache_write_1h_tokens: 1,
            output_tokens: 1,
            image_input_tokens: 1,
            image_output_tokens: 1,
        };

        for (model, mapped_model) in [
            (Some("unknown-model"), None),
            (Some("vendor/unknown-model"), Some(" ")),
            (Some("gpt-6-sol"), Some("unknown-mapped-model")),
            (None, Some("unknown-mapped-model")),
        ] {
            let cost = calculate_request_cost(&settings, model, mapped_model, None, &usage)
                .expect("fallback cost");
            assert_eq!(cost.pricing_model, "default");
            assert_eq!(cost.cost_nano_usd, 33_100);
            assert_eq!(cost.context_tier, PricingContextTier::Standard);
            assert_eq!(
                cost.breakdown,
                RequestCostBreakdown {
                    uncached_input_nano_usd: 2_000,
                    cache_read_nano_usd: 100,
                    cache_write_nano_usd: 2_500,
                    cache_write_5m_nano_usd: 2_500,
                    cache_write_1h_nano_usd: 4_000,
                    output_nano_usd: 10_000,
                    image_input_nano_usd: 2_000,
                    image_output_nano_usd: 10_000,
                }
            );
        }
        assert!(calculate_request_cost(&settings, None, Some(" "), None, &usage).is_none());
    }

    #[test]
    fn unmatched_model_prices_are_independent_of_sonnet_settings() {
        let mut settings = default_model_pricing_settings();
        let sonnet = settings
            .models
            .iter_mut()
            .find(|model| model.model_id == "claude-sonnet-5-5")
            .expect("Sonnet 5.5 pricing");
        sonnet.standard.input_nano_usd_per_token = Some(3_000);
        sonnet.price_multiplier_scaled = PRICE_MULTIPLIER_SCALE * 2;
        sonnet.service_tier_profiles.insert(
            "priority".to_string(),
            ModelPricingProfile {
                input_nano_usd_per_token: Some(6_000),
                ..ModelPricingProfile::default()
            },
        );
        sonnet.long_context = Some(LongContextPricing {
            threshold: 100,
            input_multiplier_scaled: PRICE_MULTIPLIER_SCALE * 2,
            output_multiplier_scaled: PRICE_MULTIPLIER_SCALE * 2,
        });
        let usage = BillableUsage {
            uncached_input_tokens: 101,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        for tier in [None, Some("priority"), Some("fast")] {
            let cost = calculate_request_cost(&settings, Some("unknown-model"), None, tier, &usage)
                .expect("fixed fallback cost");
            assert_eq!(cost.pricing_model, "default");
            assert_eq!(cost.cost_nano_usd, 212_000);
            assert_eq!(cost.context_tier, PricingContextTier::Standard);
        }
    }

    #[test]
    fn unmatched_models_use_default_prices_without_sonnet_entry() {
        let settings = ModelPricingSettings {
            version: "test".to_string(),
            source: None,
            models: vec![custom_model()],
        };
        let usage = BillableUsage {
            uncached_input_tokens: 1,
            output_tokens: 1,
            ..BillableUsage::default()
        };
        let cost = calculate_request_cost(&settings, Some("unknown-model"), None, None, &usage)
            .expect("fixed fallback cost");
        assert_eq!(cost.pricing_model, "default");
        assert_eq!(cost.cost_nano_usd, 12_000);

        let matched =
            calculate_request_cost(&settings, Some("openai/custom-model"), None, None, &usage)
                .expect("matched custom cost");
        assert_eq!(matched.pricing_model, "custom-model");
        assert_eq!(matched.cost_nano_usd, 300);
    }

    #[tokio::test]
    async fn backfill_prices_unmatched_models_with_current_pricing_version() {
        let pool = setup_pool().await;
        let settings = default_model_pricing_settings();
        sqlx::query(
            r#"
INSERT INTO request_logs (
  ts_ms, path, provider, upstream_id, model, mapped_model, stream, status,
  uncached_input_tokens, output_tokens, latency_ms, pricing_version
) VALUES
  (1, '/v1/responses', 'openai-response', 'test', 'unknown-model', NULL, 0, 200, 100, 10, 1, ?),
  (2, '/v1/responses', 'openai-response', 'test', 'gpt-6-sol', 'unknown-model', 0, 200, 100, 10, 1, ?);
"#,
        )
        .bind(&settings.version)
        .bind(&settings.version)
        .execute(&pool)
        .await
        .expect("insert unpriced logs");

        crate::sqlite::backfill_request_logs(&pool)
            .await
            .expect("backfill on startup");

        let rows =
            sqlx::query("SELECT cost_nano_usd, pricing_model FROM request_logs ORDER BY id;")
                .fetch_all(&pool)
                .await
                .expect("read backfilled costs");
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert_eq!(row.get::<i64, _>("cost_nano_usd"), 300_000);
            assert_eq!(row.get::<String, _>("pricing_model"), "default");
        }
    }

    #[test]
    fn missing_price_is_unpriced_but_explicit_zero_is_free() {
        let mut model = custom_model();
        model.standard.cache_write_1h_nano_usd_per_token = None;
        model.standard.cache_write_nano_usd_per_token = None;
        let settings = ModelPricingSettings {
            version: "test".to_string(),
            source: None,
            models: vec![model.clone()],
        };
        let usage = BillableUsage {
            cache_write_1h_tokens: 1,
            ..BillableUsage::default()
        };
        assert!(
            calculate_request_cost(&settings, Some("custom-model"), None, None, &usage).is_none()
        );

        model.standard.cache_write_1h_nano_usd_per_token = Some(0);
        let settings = ModelPricingSettings {
            version: "test".to_string(),
            source: None,
            models: vec![model],
        };
        assert_eq!(
            calculate_request_cost(&settings, Some("custom-model"), None, None, &usage)
                .expect("free cost")
                .cost_nano_usd,
            0
        );
    }

    #[test]
    fn remote_catalog_merge_preserves_only_explicit_overrides() {
        let mut catalog = default_model_pricing_settings();
        catalog.models = vec![custom_model()];
        let mut changed = custom_model();
        changed.standard.input_nano_usd_per_token = Some(999);
        let overrides = diff_overrides(&catalog.models, &[changed.clone()]);
        assert_eq!(overrides.upserts, vec![changed.clone()]);

        let mut updated_catalog = catalog.clone();
        updated_catalog.models[0].standard.output_nano_usd_per_token = Some(777);
        let effective = effective_settings(&updated_catalog, &overrides).expect("merge");
        assert_eq!(effective.models[0], changed);
    }

    #[tokio::test]
    async fn save_stores_overrides_and_reset_tracks_catalog() {
        let pool = setup_pool().await;
        let mut model = default_model_pricing_settings().models[0].clone();
        model.standard.input_nano_usd_per_token = Some(9_999);
        let saved = save_model_pricing_settings(
            &pool,
            ModelPricingSettingsInput {
                models: vec![model.clone()],
            },
        )
        .await
        .expect("save");
        assert!(saved.settings.version.starts_with("custom."));
        assert_eq!(saved.settings.models, vec![model]);

        let reset = reset_model_pricing_settings(&pool).await.expect("reset");
        assert_eq!(reset.settings, reset.default_settings);
    }

    #[tokio::test]
    async fn remote_catalog_storage_preserves_etag_and_cached_catalog() {
        let pool = setup_pool().await;
        let mut catalog = serde_json::from_str::<serde_json::Value>(BUNDLED_PRICING_CATALOG)
            .expect("bundled catalog json");
        catalog["version"] = serde_json::Value::String("remote.test".to_string());
        let body = serde_json::to_string(&catalog).expect("catalog body");
        let first = store_remote_catalog(&pool, &body, Some("\"pricing-v1\""))
            .await
            .expect("store catalog");
        let etag = read_remote_catalog_etag(&pool).await.expect("read etag");
        let settings = read_model_pricing_settings(&pool)
            .await
            .expect("cached settings");

        assert_eq!(first, RemoteCatalogRefresh::Updated);
        assert_eq!(etag.as_deref(), Some("\"pricing-v1\""));
        assert_eq!(settings.version, "remote.test+curated.20261027");
    }

    #[test]
    fn rejects_legacy_short_long_schema() {
        let json = r#"{
          "modelId":"legacy",
          "aliases":[],
          "priceMultiplierScaled":1000000000000,
          "short":{"inputNanoUsdPerToken":1},
          "long":null
        }"#;
        assert!(serde_json::from_str::<ModelPricingModel>(json).is_err());
    }
}
