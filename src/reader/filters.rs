use super::{Feed, Publication};
use chrono::{Duration, NaiveDate};
use std::collections::BTreeMap;

pub(super) const PERIOD_PARAM: &str = "view_period";
pub(super) const AUTHOR_PARAM: &str = "view_author";
pub(super) const SOURCE_PARAM: &str = "view_source";

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
    pub author: Option<String>,
    pub source: Option<String>,
}

impl ViewFilters {
    pub(super) fn from_params(params: &[(String, String)]) -> Self {
        Self {
            period: value(params, PERIOD_PARAM).and_then(parse_period),
            author: nonempty_value(params, AUTHOR_PARAM),
            source: nonempty_value(params, SOURCE_PARAM),
        }
    }

    pub(super) fn validated(mut self, options: &FilterOptions) -> Self {
        if self
            .author
            .as_ref()
            .is_some_and(|author| !options.authors.contains_key(author))
        {
            self.author = None;
        }
        if self
            .source
            .as_ref()
            .is_some_and(|source| !options.sources.contains_key(source))
        {
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
        let author_matches = self.author.as_ref().is_none_or(|selected| {
            publication
                .authors
                .iter()
                .any(|author| author.matched_feed && &author.filter_id == selected)
        });
        let source_matches = self.source.as_ref().is_none_or(|selected| {
            publication
                .curated_sources
                .iter()
                .any(|source| source.key.as_ref() == Some(selected))
        });

        date_matches && author_matches && source_matches
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct FilterOptions {
    pub authors: BTreeMap<String, String>,
    pub sources: BTreeMap<String, String>,
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
            for source in &publication.curated_sources {
                if let Some(key) = source.key.as_ref().filter(|key| !key.is_empty()) {
                    options
                        .sources
                        .entry(key.clone())
                        .or_insert_with(|| source.name.clone());
                }
            }
        }
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
            curated_sources: Vec::new(),
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
}
