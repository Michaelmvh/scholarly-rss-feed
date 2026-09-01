use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Top-level feeds configuration, parsed from a TOML file.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// Name of the feed served at bare `/`.
    pub default_feed: Option<String>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub feeds: HashMap<String, FeedConfig>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Settings {
    /// Default recency window in days when a feed omits `from`.
    pub from_days: Option<u32>,
}

/// A single named feed definition.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct FeedConfig {
    /// Optional RSS channel title override.
    pub title: Option<String>,
    /// Canonical people with provider-specific identifiers.
    #[serde(default)]
    pub people: Vec<PersonConfig>,
    /// Curated paper collections merged into this feed.
    #[serde(default)]
    pub curated_sources: Vec<String>,
    /// bioRxiv subject categories discovered through the native API.
    #[serde(default)]
    pub biorxiv_categories: Vec<String>,
    /// Legacy OpenAlex author IDs.
    #[serde(default)]
    pub author_ids: Vec<String>,
    /// Legacy ORCIDs resolved to OpenAlex author IDs.
    #[serde(default)]
    pub orcids: Vec<String>,
    /// Legacy author names resolved through OpenAlex search.
    #[serde(default)]
    pub authors: Vec<String>,
    /// Legacy author names used by the archived Google Scholar provider.
    #[serde(default)]
    pub google_scholar_authors: Vec<String>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub issns: Vec<String>,
    #[serde(default)]
    pub journals: Vec<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    /// Explicit earliest publication date (YYYY-MM-DD); overrides `from_days`.
    pub from: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PersonConfig {
    /// Canonical human-readable name.
    pub name: String,
    /// Precise OpenAlex author ID.
    pub openalex_id: Option<String>,
    /// Optional Google Scholar query spelling; defaults to `name`.
    pub google_scholar_name: Option<String>,
}

impl Config {
    /// Load config from `path`. A missing file yields an empty config so the
    /// server still works in pure ad-hoc-param mode.
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(contents) => match toml::from_str::<Config>(&contents) {
                Ok(config) => config,
                Err(err) => {
                    eprintln!("Failed to parse config {}: {err}", path.display());
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }
}
