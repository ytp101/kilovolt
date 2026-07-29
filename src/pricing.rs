use std::fmt;
use std::path::Path;

use chrono::NaiveDate;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Openai,
    Gemini,
}

impl Provider {
    pub fn for_model(model: &str) -> Self {
        if model.starts_with("gemini-") {
            Self::Gemini
        } else {
            Self::Openai
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Openai => formatter.write_str("openai"),
            Self::Gemini => formatter.write_str("gemini"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchType {
    Exact,
    Prefix,
}

impl fmt::Display for MatchType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact => formatter.write_str("exact"),
            Self::Prefix => formatter.write_str("prefix"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PricingSource {
    BuiltIn,
    Operator,
}

impl fmt::Display for PricingSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuiltIn => formatter.write_str("built-in-unverified"),
            Self::Operator => formatter.write_str("operator-file"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPricing {
    pub pricing: ModelPricing,
    pub provider: Provider,
    pub matched_model: String,
    pub match_type: MatchType,
    pub effective_date: Option<NaiveDate>,
    pub source: PricingSource,
}

#[derive(Debug)]
pub enum PricingError {
    Io(std::io::Error),
    InvalidJson(serde_json::Error),
    UnsupportedSchemaVersion(u64),
    EmptyModel,
    InvalidPrice,
    InvalidEffectiveDate(String),
    DuplicateExact,
    AmbiguousPattern,
    NotConfigured,
}

impl fmt::Display for PricingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "could not read pricing file: {error}"),
            Self::InvalidJson(error) => write!(formatter, "invalid pricing JSON: {error}"),
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "unsupported pricing schema version {version}")
            }
            Self::EmptyModel => formatter.write_str("pricing model must not be empty"),
            Self::InvalidPrice => {
                formatter.write_str("pricing values must be finite and non-negative")
            }
            Self::InvalidEffectiveDate(date) => {
                write!(formatter, "invalid pricing effective date {date:?}")
            }
            Self::DuplicateExact => {
                formatter.write_str("duplicate exact pricing entry for provider/model")
            }
            Self::AmbiguousPattern => {
                formatter.write_str("duplicate or ambiguous pricing prefix pattern")
            }
            Self::NotConfigured => formatter.write_str("model pricing is not configured"),
        }
    }
}

impl std::error::Error for PricingError {}

#[derive(Clone, Debug)]
struct PricingEntry {
    provider: Provider,
    model: String,
    match_type: MatchType,
    pricing: ModelPricing,
    effective_date: Option<NaiveDate>,
    source: PricingSource,
}

#[derive(Debug, serde::Deserialize)]
struct PricingFile {
    version: u64,
    entries: Vec<PricingFileEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct PricingFileEntry {
    provider: Provider,
    model: String,
    #[serde(rename = "match")]
    match_type: MatchType,
    input_usd_per_million_tokens: f64,
    output_usd_per_million_tokens: f64,
    effective_date: String,
}

#[derive(Clone, Debug)]
pub struct PricingRegistry {
    built_in: Vec<PricingEntry>,
    operator: Vec<PricingEntry>,
}

impl Default for PricingRegistry {
    fn default() -> Self {
        Self::built_in()
    }
}

impl PricingRegistry {
    pub fn built_in() -> Self {
        // These preserve the repository's legacy Phase 3 values. Their provider
        // effective dates were not independently verified, so the metadata
        // intentionally records `None` rather than inventing a date.
        let entries = [
            (
                Provider::Openai,
                "gpt-4o-mini",
                MatchType::Prefix,
                0.15,
                0.60,
            ),
            (Provider::Openai, "gpt-4o", MatchType::Prefix, 5.00, 15.00),
            (Provider::Openai, "gpt-4", MatchType::Prefix, 30.00, 60.00),
            (
                Provider::Openai,
                "gpt-3.5-turbo",
                MatchType::Prefix,
                0.50,
                1.50,
            ),
            (
                Provider::Gemini,
                "gemini-1.5-flash",
                MatchType::Prefix,
                0.075,
                0.30,
            ),
            (
                Provider::Gemini,
                "gemini-1.5-pro",
                MatchType::Prefix,
                1.25,
                5.00,
            ),
        ]
        .into_iter()
        .map(
            |(provider, model, match_type, input_per_million, output_per_million)| PricingEntry {
                provider,
                model: model.to_string(),
                match_type,
                pricing: ModelPricing {
                    input_cost_per_token: input_per_million / 1_000_000.0,
                    output_cost_per_token: output_per_million / 1_000_000.0,
                },
                effective_date: None,
                source: PricingSource::BuiltIn,
            },
        )
        .collect();

        Self {
            built_in: entries,
            operator: Vec::new(),
        }
    }

    pub fn load(path: Option<&Path>) -> Result<Self, PricingError> {
        let mut registry = Self::built_in();
        if let Some(path) = path {
            let contents = std::fs::read_to_string(path).map_err(PricingError::Io)?;
            registry.operator = Self::parse_operator_entries(&contents)?;
        }
        Ok(registry)
    }

    fn parse_operator_entries(contents: &str) -> Result<Vec<PricingEntry>, PricingError> {
        let file: PricingFile =
            serde_json::from_str(contents).map_err(PricingError::InvalidJson)?;
        if file.version != 1 {
            return Err(PricingError::UnsupportedSchemaVersion(file.version));
        }

        let mut entries = Vec::with_capacity(file.entries.len());
        for raw in file.entries {
            let model = raw.model.trim();
            if model.is_empty() {
                return Err(PricingError::EmptyModel);
            }
            validate_prices(
                raw.input_usd_per_million_tokens,
                raw.output_usd_per_million_tokens,
            )?;
            let effective_date = NaiveDate::parse_from_str(&raw.effective_date, "%Y-%m-%d")
                .map_err(|_| PricingError::InvalidEffectiveDate(raw.effective_date.clone()))?;
            let duplicate = entries.iter().any(|entry: &PricingEntry| {
                entry.provider == raw.provider
                    && entry.model == model
                    && entry.match_type == raw.match_type
            });
            if duplicate {
                return Err(match raw.match_type {
                    MatchType::Exact => PricingError::DuplicateExact,
                    MatchType::Prefix => PricingError::AmbiguousPattern,
                });
            }
            entries.push(PricingEntry {
                provider: raw.provider,
                model: model.to_string(),
                match_type: raw.match_type,
                pricing: ModelPricing {
                    input_cost_per_token: raw.input_usd_per_million_tokens / 1_000_000.0,
                    output_cost_per_token: raw.output_usd_per_million_tokens / 1_000_000.0,
                },
                effective_date: Some(effective_date),
                source: PricingSource::Operator,
            });
        }
        Ok(entries)
    }

    pub fn resolve(
        &self,
        provider: Provider,
        model: &str,
    ) -> Result<ResolvedPricing, PricingError> {
        if model.trim().is_empty() {
            return Err(PricingError::EmptyModel);
        }

        select_entry(&self.operator, provider, model)
            .or_else(|| select_entry(&self.built_in, provider, model))
            .map(|entry| ResolvedPricing {
                pricing: entry.pricing,
                provider: entry.provider,
                matched_model: entry.model.clone(),
                match_type: entry.match_type,
                effective_date: entry.effective_date,
                source: entry.source,
            })
            .ok_or(PricingError::NotConfigured)
    }

    pub fn operator_entry_count(&self) -> usize {
        self.operator.len()
    }

    pub fn built_in_entry_count(&self) -> usize {
        self.built_in.len()
    }
}

fn select_entry<'a>(
    entries: &'a [PricingEntry],
    provider: Provider,
    model: &str,
) -> Option<&'a PricingEntry> {
    entries
        .iter()
        .find(|entry| {
            entry.provider == provider
                && entry.match_type == MatchType::Exact
                && entry.model == model
        })
        .or_else(|| {
            entries
                .iter()
                .filter(|entry| {
                    entry.provider == provider
                        && entry.match_type == MatchType::Prefix
                        && model.starts_with(&entry.model)
                })
                .max_by_key(|entry| entry.model.len())
        })
}

fn validate_prices(input: f64, output: f64) -> Result<(), PricingError> {
    if input.is_finite() && output.is_finite() && input >= 0.0 && output >= 0.0 {
        Ok(())
    } else {
        Err(PricingError::InvalidPrice)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MatchType, PricingError, PricingRegistry, PricingSource, Provider, validate_prices,
    };

    fn operator_file(entries: &str) -> String {
        format!(r#"{{"version":1,"entries":[{entries}]}}"#)
    }

    #[test]
    fn exact_known_model_resolves_and_unknown_fails_closed() {
        let registry = PricingRegistry::built_in();
        let known = registry
            .resolve(Provider::Openai, "gpt-4o-mini-2024-07-18")
            .expect("known built-in prefix should resolve");
        assert_eq!(known.matched_model, "gpt-4o-mini");
        assert_eq!(known.source, PricingSource::BuiltIn);
        assert_eq!(known.match_type, MatchType::Prefix);
        assert!(matches!(
            registry.resolve(Provider::Openai, "unconfigured-model"),
            Err(PricingError::NotConfigured)
        ));
    }

    #[test]
    fn operator_exact_override_precedes_built_in() {
        let contents = operator_file(
            r#"{"provider":"openai","model":"gpt-4o-mini","match":"exact","input_usd_per_million_tokens":1.25,"output_usd_per_million_tokens":2.5,"effective_date":"2026-01-15"}"#,
        );
        let mut registry = PricingRegistry::built_in();
        registry.operator =
            PricingRegistry::parse_operator_entries(&contents).expect("operator file should parse");
        let resolved = registry
            .resolve(Provider::Openai, "gpt-4o-mini")
            .expect("operator exact should resolve");
        assert_eq!(resolved.source, PricingSource::Operator);
        assert_eq!(resolved.pricing.input_cost_per_token, 1.25 / 1_000_000.0);
        assert_eq!(
            resolved.effective_date.map(|date| date.to_string()),
            Some("2026-01-15".to_string())
        );
    }

    #[test]
    fn longest_operator_prefix_has_deterministic_precedence() {
        let contents = operator_file(
            r#"{"provider":"openai","model":"custom-","match":"prefix","input_usd_per_million_tokens":1,"output_usd_per_million_tokens":2,"effective_date":"2026-01-01"},{"provider":"openai","model":"custom-special-","match":"prefix","input_usd_per_million_tokens":3,"output_usd_per_million_tokens":4,"effective_date":"2026-01-02"}"#,
        );
        let mut registry = PricingRegistry::built_in();
        registry.operator =
            PricingRegistry::parse_operator_entries(&contents).expect("operator file should parse");
        let resolved = registry
            .resolve(Provider::Openai, "custom-special-v1")
            .expect("specific prefix should resolve");
        assert_eq!(resolved.matched_model, "custom-special-");
        assert_eq!(resolved.pricing.input_cost_per_token, 3.0 / 1_000_000.0);
    }

    #[test]
    fn duplicate_and_ambiguous_entries_are_rejected() {
        let duplicate_exact = operator_file(
            r#"{"provider":"openai","model":"custom","match":"exact","input_usd_per_million_tokens":1,"output_usd_per_million_tokens":2,"effective_date":"2026-01-01"},{"provider":"openai","model":"custom","match":"exact","input_usd_per_million_tokens":3,"output_usd_per_million_tokens":4,"effective_date":"2026-01-02"}"#,
        );
        assert!(matches!(
            PricingRegistry::parse_operator_entries(&duplicate_exact),
            Err(PricingError::DuplicateExact)
        ));

        let duplicate_prefix = operator_file(
            r#"{"provider":"openai","model":"custom-","match":"prefix","input_usd_per_million_tokens":1,"output_usd_per_million_tokens":2,"effective_date":"2026-01-01"},{"provider":"openai","model":"custom-","match":"prefix","input_usd_per_million_tokens":3,"output_usd_per_million_tokens":4,"effective_date":"2026-01-02"}"#,
        );
        assert!(matches!(
            PricingRegistry::parse_operator_entries(&duplicate_prefix),
            Err(PricingError::AmbiguousPattern)
        ));
    }

    #[test]
    fn invalid_values_dates_schema_and_types_are_rejected() {
        assert!(validate_prices(-1.0, 1.0).is_err());
        assert!(validate_prices(f64::INFINITY, 1.0).is_err());

        let invalid_date = operator_file(
            r#"{"provider":"openai","model":"custom","match":"exact","input_usd_per_million_tokens":1,"output_usd_per_million_tokens":2,"effective_date":"not-a-date"}"#,
        );
        assert!(matches!(
            PricingRegistry::parse_operator_entries(&invalid_date),
            Err(PricingError::InvalidEffectiveDate(_))
        ));

        assert!(matches!(
            PricingRegistry::parse_operator_entries(r#"{"version":2,"entries":[]}"#),
            Err(PricingError::UnsupportedSchemaVersion(2))
        ));
        assert!(PricingRegistry::parse_operator_entries(
            &operator_file(
                r#"{"provider":"openai","model":"","match":"exact","input_usd_per_million_tokens":1,"output_usd_per_million_tokens":2,"effective_date":"2026-01-01"}"#,
            )
        )
        .is_err());
        assert!(PricingRegistry::parse_operator_entries(
            &operator_file(
                r#"{"provider":"openai","model":"x","match":"glob","input_usd_per_million_tokens":1,"output_usd_per_million_tokens":2,"effective_date":"2026-01-01"}"#,
            )
        )
        .is_err());
    }
}
