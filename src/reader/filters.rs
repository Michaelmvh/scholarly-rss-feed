use super::{Feed, Publication};
use chrono::{Duration, NaiveDate};
use std::collections::BTreeMap;

pub(super) const PERIOD_PARAM: &str = "view_period";
pub(super) const AUTHOR_PARAM: &str = "view_author";
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
    pub source: Option<String>,
}

impl ViewFilters {
    pub(super) fn from_params(params: &[(String, String)]) -> Self {
        Self {
            period: value(params, PERIOD_PARAM).and_then(parse_period),
            authors: values(params, AUTHOR_PARAM),
            source: nonempty_value(params, SOURCE_PARAM),
        }
    }

    pub(super) fn validated(mut self, options: &FilterOptions) -> Self {
        self.authors
            .retain(|author| options.authors.contains_key(author));
        if self.source.as_ref().is_some_and(|source| {
            if source == EXCLUDE_CURATED_ONLY {
                !options.can_exclude_collection_only
            } else {
                !options.sources.contains_key(source)
            }
        }) {
            self.source = None;
        }
        self
    }

    pub(super) fn matches_on(&self, publication: &Publication, today: NaiveDate) -> bool {
        let date_matches = self.period.is_none_or(|period| {
            publication
                .publication_date
                .as_deref()
                .and_then(parse_date)
                .is_some_and(|date| date >= today - Duration::days(period.days()))
        });
        let author_matches = self.authors.is_empty()
            || self.authors.iter().any(|selected| {
                publication
                    .authors
                    .iter()
                    .any(|author| author.matched_feed && &author.filter_id == selected)
            });
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
    pub sources: BTreeMap<String, String>,
    pub can_exclude_collection_only: bool,
}

impl FilterOptions {
    pub(super) fn from_feed(feed: &Feed) -> Self {
        let mut options = Self::default();
        for publication in &feed.publications {
            for author in &publication.authors {
                if author.matched_feed && !author.filter_id.is_empty() {
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
    matches!(name, PERIOD_PARAM | AUTHOR_PARAM | SOURCE_PARAM)
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

        assert_eq!(ViewFilters::from_params(&params), ViewFilters::default());
    }

    #[test]
    fn repeated_author_filters_are_deduplicated() {
        let params = vec![
            (AUTHOR_PARAM.to_string(), "grace hopper".to_string()),
            (AUTHOR_PARAM.to_string(), "ada lovelace".to_string()),
            (AUTHOR_PARAM.to_string(), "ada lovelace".to_string()),
        ];

        assert_eq!(
            ViewFilters::from_params(&params).authors,
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
