use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySourceKind {
    OpenAlex,
    GoogleScholar,
    Biorxiv,
    Arxiv,
    CuratedCollection,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscoverySource {
    pub kind: DiscoverySourceKind,
    pub key: String,
    pub label: String,
    pub url: Option<String>,
}

impl DiscoverySource {
    pub fn openalex() -> Self {
        Self {
            kind: DiscoverySourceKind::OpenAlex,
            key: "openalex".to_string(),
            label: "OpenAlex".to_string(),
            url: Some("https://openalex.org".to_string()),
        }
    }

    pub fn google_scholar() -> Self {
        Self {
            kind: DiscoverySourceKind::GoogleScholar,
            key: "google-scholar".to_string(),
            label: "Google Scholar".to_string(),
            url: Some("https://scholar.google.com".to_string()),
        }
    }

    pub fn curated_collection(key: String, label: String, url: String) -> Self {
        Self {
            kind: DiscoverySourceKind::CuratedCollection,
            key,
            label,
            url: Some(url),
        }
    }

    pub fn is_curated_collection(&self) -> bool {
        self.kind == DiscoverySourceKind::CuratedCollection
    }
}
