use super::{Feed, Publication};
use chrono::{Duration, NaiveDate};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const PERIOD_PARAM: &str = "view_period";
pub(crate) const AUTHOR_PARAM: &str = "view_author";
pub(super) const AUTHOR_MODE_PARAM: &str = "view_authors";
pub(super) const AUTHOR_SELECTION_PARAM: &str = "view_author_selection";
pub(super) const AUTHOR_OPTIONS_PARAM: &str = "view_author_options";
pub(super) const SOURCE_PARAM: &str = "view_source";
pub(super) const EXCLUDE_CURATED_ONLY: &str = "exclude-curated-only";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Period {
    Days30,
    Days90,
    Year1,
}

impl Period {
    pub(super) fn value(self) -> &'static str {
        match self {
            Self::Days30 => "30d",
            Self::Days90 => "90d",
            Self::Year1 => "1y",
        }
    }

    fn days(self) -> i64 {
        match self {
            Self::Days30 => 30,
            Self::Days90 => 90,
            Self::Year1 => 365,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ViewFilters {
    pub period: Option<Period>,
    pub authors: Vec<String>,
    pub author_selection: AuthorSelection,
    pub source: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum AuthorSelection {
    #[default]
    Default,
    All,
    Custom,
}

impl ViewFilters {
    pub(super) fn from_params(params: &[(String, String)], options: &FilterOptions) -> Self {
        let authors = values(params, AUTHOR_PARAM);
        let had_authors = !authors.is_empty();
        // Compare the submitted checkboxes with the form's original selection, not
        // current options: switching paper sources can change the available authors.
        let selection_fingerprint = author_selection_fingerprint(&authors);
        let unchanged_selection = value(params, AUTHOR_SELECTION_PARAM)
            .is_none_or(|fingerprint| selection_fingerprint == fingerprint);
        let selected_all = !unchanged_selection
            && had_authors
            && value(params, AUTHOR_OPTIONS_PARAM)
                .is_some_and(|fingerprint| selection_fingerprint == fingerprint);
        let authors = authors
            .into_iter()
            .filter(|author| options.authors.contains_key(author))
            .collect::<Vec<_>>();
        let author_selection = match value(params, AUTHOR_MODE_PARAM) {
            Some("all") if unchanged_selection => AuthorSelection::All,
            Some("default") if unchanged_selection => AuthorSelection::Default,
            Some("custom" | "default" | "all") if selected_all => AuthorSelection::All,
            Some("default" | "custom" | "all") => AuthorSelection::Custom,
            _ if had_authors && authors.is_empty() => AuthorSelection::Default,
            _ if !authors.is_empty() => AuthorSelection::Custom,
            _ => AuthorSelection::Default,
        };
        let mut filters = Self {
            period: value(params, PERIOD_PARAM).and_then(parse_period),
            authors,
            author_selection,
            source: nonempty_value(params, SOURCE_PARAM),
        };
        if filters.author_selection != AuthorSelection::Custom {
            filters.authors.clear();
        }
        if filters.source.as_ref().is_some_and(|source| {
            if source == EXCLUDE_CURATED_ONLY {
                !options.can_exclude_collection_only
            } else {
                !options.sources.contains_key(source)
            }
        }) {
            filters.source = None;
        }
        filters
    }

    pub(super) fn matches_on(&self, publication: &Publication, today: NaiveDate) -> bool {
        let date_matches = self.period.is_none_or(|period| {
            publication
                .publication_date
                .as_deref()
                .and_then(parse_date)
                .is_some_and(|date| date >= today - Duration::days(period.days()))
        });
        let author_matches = match self.author_selection {
            AuthorSelection::Default => {
                !publication.authors.iter().any(|author| author.matched_feed)
                    || publication
                        .authors
                        .iter()
                        .any(|author| author.matched_feed && !author.optional)
            }
            AuthorSelection::All => true,
            AuthorSelection::Custom => self.authors.iter().any(|selected| {
                publication
                    .authors
                    .iter()
                    .any(|author| author.matched_feed && &author.filter_id == selected)
            }),
        };
        let source_matches = self.source.as_ref().is_none_or(|selected| {
            if selected == EXCLUDE_CURATED_ONLY {
                publication
                    .discovery_sources
                    .iter()
                    .any(|source| !source.is_curated_collection())
            } else {
                publication
                    .discovery_sources
                    .iter()
                    .any(|source| source.is_curated_collection() && &source.key == selected)
            }
        });

        date_matches && author_matches && source_matches
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct FilterOptions {
    pub authors: BTreeMap<String, String>,
    pub optional_authors: BTreeSet<String>,
    pub sources: BTreeMap<String, String>,
    pub can_exclude_collection_only: bool,
}

impl FilterOptions {
    pub(super) fn default_authors(&self) -> Vec<String> {
        self.authors
            .keys()
            .filter(|id| !self.optional_authors.contains(*id))
            .cloned()
            .collect()
    }

    pub(super) fn from_feed(feed: &Feed) -> Self {
        let mut options = Self::default();
        let mut non_optional_authors = BTreeSet::new();
        for publication in &feed.publications {
            for author in &publication.authors {
                if author.matched_feed && !author.filter_id.is_empty() {
                    if author.optional {
                        options.optional_authors.insert(author.filter_id.clone());
                    } else {
                        non_optional_authors.insert(author.filter_id.clone());
                    }
                    options
                        .authors
                        .entry(author.filter_id.clone())
                        .or_insert_with(|| author.name.clone());
                }
            }
            for source in publication
                .discovery_sources
                .iter()
                .filter(|source| source.is_curated_collection())
            {
                if !source.key.is_empty() {
                    options
                        .sources
                        .entry(source.key.clone())
                        .or_insert_with(|| source.label.clone());
                }
            }
        }
        options
            .optional_authors
            .retain(|id| !non_optional_authors.contains(id));
        options.can_exclude_collection_only = feed.publications.iter().any(|publication| {
            publication
                .discovery_sources
                .iter()
                .any(|source| source.is_curated_collection())
        }) && feed.publications.iter().any(|publication| {
            publication
                .discovery_sources
                .iter()
                .any(|source| !source.is_curated_collection())
        });
        options
    }
}

pub(super) fn is_view_param(name: &str) -> bool {
    matches!(
        name,
        PERIOD_PARAM
            | AUTHOR_PARAM
            | AUTHOR_MODE_PARAM
            | AUTHOR_SELECTION_PARAM
            | AUTHOR_OPTIONS_PARAM
            | SOURCE_PARAM
    )
}

pub(super) fn author_selection_fingerprint(authors: &[String]) -> String {
    let authors = authors.iter().collect::<BTreeSet<_>>();
    let serialized = serde_json::to_vec(&authors).expect("author IDs serialize as JSON");
    format!("{:x}", Sha256::digest(serialized))
}

fn value<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn nonempty_value(params: &[(String, String)], name: &str) -> Option<String> {
    value(params, name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn values(params: &[(String, String)], name: &str) -> Vec<String> {
    let mut values = params
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn parse_period(value: &str) -> Option<Period> {
    match value {
        "30d" => Some(Period::Days30),
        "90d" => Some(Period::Days90),
        "1y" => Some(Period::Year1),
        _ => None,
    }
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::DiscoverySource;
    use crate::reader::{Author, Publication};

    #[test]
    fn selection_fingerprints_are_bounded_and_order_independent() {
        let fingerprint = author_selection_fingerprint(&["a".to_string(), "bc".to_string()]);
        assert_eq!(fingerprint.len(), 64);
        assert_eq!(
            fingerprint,
            author_selection_fingerprint(&["bc".to_string(), "a".to_string(), "a".to_string(),])
        );
        assert_ne!(
            fingerprint,
            author_selection_fingerprint(&["ab".to_string(), "c".to_string()])
        );
        assert_eq!(author_selection_fingerprint(&[]).len(), 64);
    }

    fn publication(date: Option<&str>) -> Publication {
        Publication {
            id: None,
            title: "Publication".to_string(),
            link: None,
            pdf_url: None,
            publication_date: date.map(str::to_string),
            collection_date: None,
            venue: None,
            authors: vec![Author {
                name: "Ada Lovelace".to_string(),
                filter_id: "ada-lovelace".to_string(),
                matched_feed: true,
                optional: false,
            }],
            abstract_text: None,
            discovery_sources: vec![DiscoverySource::openalex()],
            curated_categories: Vec::new(),
        }
    }

    #[test]
    fn period_filters_use_publication_date_and_exclude_undated_works() {
        let filters = ViewFilters {
            period: Some(Period::Days30),
            ..ViewFilters::default()
        };
        let today = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();

        assert!(filters.matches_on(&publication(Some("2026-08-02")), today));
        assert!(!filters.matches_on(&publication(Some("2026-08-01")), today));
        assert!(!filters.matches_on(&publication(None), today));
    }

    #[test]
    fn malformed_period_is_ignored() {
        let params = vec![(PERIOD_PARAM.to_string(), "recent-ish".to_string())];

        assert_eq!(
            ViewFilters::from_params(&params, &FilterOptions::default()),
            ViewFilters::default()
        );
    }

    #[test]
    fn repeated_author_filters_are_deduplicated() {
        let params = vec![
            (AUTHOR_PARAM.to_string(), "grace hopper".to_string()),
            (AUTHOR_PARAM.to_string(), "ada lovelace".to_string()),
            (AUTHOR_PARAM.to_string(), "ada lovelace".to_string()),
        ];

        assert_eq!(
            ViewFilters::from_params(
                &params,
                &FilterOptions {
                    authors: BTreeMap::from([
                        ("ada lovelace".to_string(), "Ada Lovelace".to_string()),
                        ("grace hopper".to_string(), "Grace Hopper".to_string()),
                    ]),
                    ..FilterOptions::default()
                }
            )
            .authors,
            vec!["ada lovelace", "grace hopper"]
        );
    }

    #[test]
    fn exclude_collection_only_keeps_provider_collection_overlap() {
        let mut provider_overlap = publication(Some("2026-08-20"));
        provider_overlap
            .discovery_sources
            .push(DiscoverySource::curated_collection(
                "collection".to_string(),
                "Collection".to_string(),
                "https://example.com/collection".to_string(),
            ));
        let mut collection_only = provider_overlap.clone();
        collection_only
            .discovery_sources
            .retain(DiscoverySource::is_curated_collection);
        let filters = ViewFilters {
            source: Some(EXCLUDE_CURATED_ONLY.to_string()),
            ..ViewFilters::default()
        };
        let today = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();

        assert!(filters.matches_on(&provider_overlap, today));
        assert!(!filters.matches_on(&collection_only, today));
    }
}
