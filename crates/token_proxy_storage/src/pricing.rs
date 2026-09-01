use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PRICE_MULTIPLIER_SCALE: u64 = 1_000_000_000_000;
const BUNDLED_PRICING_CATALOG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/model-pricing.json"
));
const CURATED_PRICING_OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/curated-model-pricing.json"
));
const SETTINGS_ROW_ID: i64 = 1;

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

pub async fn backfill_request_log_costs(pool: &SqlitePool) -> Result<(), String> {
    let settings = read_model_pricing_settings(pool).await?;
    backfill_request_log_costs_with_settings(pool, &settings).await
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
    let model_price = find_model_price(settings, effective_model)?;
    let normalized_service_tier = normalize_service_tier(service_tier);
    let selected_profile = model_price
        .service_tier_profiles
        .get(normalized_service_tier.as_str());
    let long_context = model_price
        .long_context
        .as_ref()
        .filter(|long| usage.total_input_tokens() > long.threshold);
    let input_long_multiplier = long_context
        .map(|long| long.input_multiplier_scaled)
        .unwrap_or(PRICE_MULTIPLIER_SCALE);
    let output_long_multiplier = long_context
        .map(|long| long.output_multiplier_scaled)
        .unwrap_or(PRICE_MULTIPLIER_SCALE);
    let profile = ResolvedProfile::new(&model_price.standard, selected_profile);

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
    apply_breakdown_multiplier(&mut breakdown, model_price.price_multiplier_scaled);

    Some(RequestCost {
        cost_nano_usd: breakdown.total(),
        pricing_version: settings.version.clone(),
        pricing_model: model_price.model_id.clone(),
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
        .map_err(|err| format!("Failed to commit request log cost backfill: {err}"))
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
            "catalog.69854741.b88b66df+curated.20260901"
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
        assert!(settings.version.contains("+curated.20260901"));
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
        ] {
            for (service_tier, expected_cost) in [
                (None, expected[0]),
                (Some("priority"), expected[1]),
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
        assert_eq!(settings.version, "remote.test+curated.20260901");
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
