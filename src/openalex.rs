use crate::provenance::DiscoverySource;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const API_BASE: &str = "https://api.openalex.org";

/// A single author record from `/authors`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Author {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AuthorsResponse {
    pub results: Vec<Author>,
}

/// A source (journal, repository, etc.) record from `/sources`.
#[derive(Debug, Deserialize)]
pub struct SourceRecord {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SourcesResponse {
    pub results: Vec<SourceRecord>,
}

/// A single work record from `/works`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Work {
    pub id: Option<String>,
    pub doi: Option<String>,
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub publication_date: Option<String>,
    #[serde(default)]
    pub latest_version_date: Option<String>,
    #[serde(default)]
    pub collection_date: Option<CollectionDate>,
    pub cited_by_count: Option<u64>,
    pub authorships: Option<Vec<Authorship>>,
    pub primary_location: Option<Location>,
    pub best_oa_location: Option<Location>,
    pub abstract_inverted_index: Option<HashMap<String, Vec<u32>>>,
    #[serde(default)]
    pub abstract_text_override: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub full_text_url: Option<String>,
    #[serde(default)]
    pub published_doi: Option<String>,
    #[serde(default)]
    pub alternate_links: Vec<String>,
    #[serde(default)]
    pub matched_author_names: Vec<String>,
    #[serde(default)]
    pub discovery_sources: Vec<DiscoverySource>,
    #[serde(default)]
    pub curated_categories: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CollectionDate {
    pub date: String,
    pub commit_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Authorship {
    pub author: Option<Author>,
    pub raw_author_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Location {
    pub landing_page_url: Option<String>,
    pub pdf_url: Option<String>,
    pub source: Option<Source>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Source {
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorksResponse {
    pub results: Vec<Work>,
}

impl Work {
    pub fn add_discovery_source(&mut self, source: DiscoverySource) {
        if !self.discovery_sources.contains(&source) {
            self.discovery_sources.push(source);
            self.discovery_sources.sort();
        }
    }

    pub fn curated_discovery_sources(&self) -> impl Iterator<Item = &DiscoverySource> {
        self.discovery_sources
            .iter()
            .filter(|source| source.is_curated_collection())
    }

    pub fn retention_date(&self) -> Option<&str> {
        [
            self.latest_version_date.as_deref(),
            self.publication_date.as_deref(),
        ]
        .into_iter()
        .flatten()
        .max()
        .or_else(|| self.collection_date.as_ref().map(|date| date.date.as_str()))
    }

    /// Best available human-readable title.
    pub fn best_title(&self) -> String {
        self.title
            .clone()
            .or_else(|| self.display_name.clone())
            .unwrap_or_else(|| String::from("Untitled"))
    }

    /// Best available link for the work: DOI, then landing page, then OpenAlex id.
    pub fn best_link(&self) -> Option<String> {
        if let Some(doi) = &self.doi {
            return Some(doi.clone());
        }
        if let Some(loc) = &self.primary_location {
            if let Some(url) = &loc.landing_page_url {
                return Some(url.clone());
            }
        }
        self.id.clone()
    }

    /// Author display names.
    pub fn author_names(&self) -> Vec<String> {
        self.authorships
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|authorship| {
                authorship
                    .author
                    .as_ref()
                    .and_then(|author| author.display_name.clone())
                    .or_else(|| authorship.raw_author_name.clone())
            })
            .collect()
    }

    /// Venue (journal / repository) name.
    pub fn venue(&self) -> Option<String> {
        self.primary_location
            .as_ref()
            .and_then(|loc| loc.source.as_ref())
            .and_then(|s| s.display_name.clone())
    }

    /// Open-access PDF url, if any.
    pub fn oa_pdf_url(&self) -> Option<String> {
        self.best_oa_location
            .as_ref()
            .and_then(|loc| loc.pdf_url.clone())
            .or_else(|| {
                self.primary_location
                    .as_ref()
                    .and_then(|loc| loc.pdf_url.clone())
            })
    }

    /// Reconstruct the abstract text from OpenAlex's inverted index.
    pub fn abstract_text(&self) -> Option<String> {
        if self.abstract_text_override.is_some() {
            return self.abstract_text_override.clone();
        }
        let index = self.abstract_inverted_index.as_ref()?;
        if index.is_empty() {
            return None;
        }
        let mut positions: Vec<(u32, &str)> = Vec::new();
        for (word, locs) in index {
            for &pos in locs {
                positions.push((pos, word.as_str()));
            }
        }
        positions.sort_by_key(|(pos, _)| *pos);
        let text = positions
            .into_iter()
            .map(|(_, word)| word)
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

/// Normalize any OpenAlex id or url (author `A…`, source `S…`, topic `T…`) to its bare
/// id form by stripping the `https://openalex.org/` prefix.
pub fn normalize_id(raw: &str) -> String {
    raw.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(raw)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{DiscoverySource, DiscoverySourceKind};

    #[test]
    fn serializes_structured_provenance() {
        let work: Work = serde_json::from_value(serde_json::json!({
            "id": "curated:1",
            "discovery_sources": [{
                "kind": "curated_collection",
                "key": "collection",
                "label": "Collection",
                "url": "https://example.com/collection"
            }]
        }))
        .unwrap();

        assert_eq!(
            work.discovery_sources,
            vec![DiscoverySource::curated_collection(
                "collection".to_string(),
                "Collection".to_string(),
                "https://example.com/collection".to_string()
            )]
        );
        assert!(work
            .discovery_sources
            .iter()
            .any(|source| source.kind == DiscoverySourceKind::CuratedCollection));

        let serialized = serde_json::to_value(work).unwrap();
        assert!(serialized.get("discovery_sources").is_some());
    }

    #[test]
    fn adding_a_discovery_source_is_idempotent() {
        let mut work: Work = serde_json::from_value(serde_json::json!({"id": "W1"})).unwrap();

        work.add_discovery_source(DiscoverySource::openalex());
        work.add_discovery_source(DiscoverySource::openalex());

        assert_eq!(work.discovery_sources, vec![DiscoverySource::openalex()]);
    }
}
