use crate::openalex::{normalize_id, Authorship, Work};
use std::collections::{HashMap, HashSet};

/// A signature that identifies a work well enough to treat two records as versions of
/// each other. Two works are grouped when any of their keys match.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum VersionKey {
    Doi(String),
    AuthorIds(String, Vec<String>),
    AuthorNames(String, Vec<String>),
}

/// Merge two result sets, deduplicating exact records and grouping publication versions.
pub(crate) fn merge_works(primary: Vec<Work>, secondary: Vec<Work>) -> Vec<Work> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut version_groups: HashMap<VersionKey, usize> = HashMap::new();
    let mut works: Vec<Work> = Vec::new();

    for work in primary.into_iter().chain(secondary) {
        if let Some(index) = work.id.as_ref().and_then(|id| seen.get(id)).copied() {
            merge_work_version(&mut works[index], work);
            continue;
        }

        let keys = version_keys(&work);
        let group = keys.iter().find_map(|key| version_groups.get(key).copied());

        match group {
            Some(index) => {
                if let Some(id) = &work.id {
                    seen.insert(id.clone(), index);
                }
                merge_work_version(&mut works[index], work);
                for key in keys {
                    version_groups.entry(key).or_insert(index);
                }
            }
            None => {
                let index = works.len();
                if let Some(id) = &work.id {
                    seen.insert(id.clone(), index);
                }
                for key in keys {
                    version_groups.insert(key, index);
                }
                works.push(work);
            }
        }
    }

    works.sort_by(|left, right| sort_date(right).cmp(sort_date(left)));

    works
}

fn sort_date(work: &Work) -> &str {
    work.publication_date
        .as_deref()
        .or_else(|| {
            work.collection_date
                .as_ref()
                .map(|collection_date| collection_date.date.as_str())
        })
        .unwrap_or("")
}

pub(crate) fn version_keys(work: &Work) -> Vec<VersionKey> {
    let mut keys = Vec::new();

    if let Some(doi) = work.doi.as_deref().map(normalize_doi) {
        if !doi.is_empty() {
            keys.push(VersionKey::Doi(doi));
        }
    }
    if let Some(arxiv_id) = work.id.as_deref().and_then(|id| id.strip_prefix("arxiv:")) {
        keys.push(VersionKey::Doi(format!(
            "10.48550/arxiv.{}",
            arxiv_id.to_ascii_lowercase()
        )));
    }

    let title = work
        .title
        .as_ref()
        .or(work.display_name.as_ref())
        .map(|title| normalize_title(title))
        .filter(|title| !title.is_empty());

    let (Some(title), Some(authorships)) = (title, work.authorships.as_ref()) else {
        return keys;
    };
    if authorships.is_empty() {
        return keys;
    }

    let author_ids = authorships
        .iter()
        .map(|authorship| {
            authorship
                .author
                .as_ref()?
                .id
                .as_ref()
                .map(|id| normalize_id(id))
        })
        .collect::<Option<Vec<_>>>();
    if let Some(mut author_ids) = author_ids {
        author_ids.sort();
        author_ids.dedup();
        if !author_ids.is_empty() {
            keys.push(VersionKey::AuthorIds(title.clone(), author_ids));
        }
    }

    let name_sources: [fn(&Authorship) -> Option<&str>; 3] = [
        |authorship| {
            authorship
                .author
                .as_ref()
                .and_then(|author| author.display_name.as_deref())
                .or(authorship.raw_author_name.as_deref())
        },
        |authorship| {
            authorship
                .author
                .as_ref()
                .and_then(|author| author.display_name.as_deref())
        },
        |authorship| authorship.raw_author_name.as_deref(),
    ];

    for source in name_sources {
        let author_names = authorships
            .iter()
            .map(|authorship| {
                let name = normalize_author_name(source(authorship)?);
                (!name.is_empty()).then_some(name)
            })
            .collect::<Option<Vec<_>>>();

        if let Some(mut author_names) = author_names {
            author_names.sort();
            author_names.dedup();
            if !author_names.is_empty() {
                let key = VersionKey::AuthorNames(title.clone(), author_names);
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
    }

    keys
}

/// Mark authors using normalized exact-name matching. Canonical names and explicit
/// provider aliases are supplied by configuration resolution.
pub(crate) fn mark_authors_by_name(works: &mut [Work], tracked_names: &[String]) {
    let tracked_names = tracked_names
        .iter()
        .map(|name| normalize_author_name(name))
        .filter(|name| !name.is_empty())
        .collect::<HashSet<_>>();
    if tracked_names.is_empty() {
        return;
    }

    for work in works {
        let Some(authorships) = work.authorships.as_deref() else {
            continue;
        };
        for authorship in authorships {
            let display_name = authorship
                .author
                .as_ref()
                .and_then(|author| author.display_name.as_ref());
            let raw_name = authorship.raw_author_name.as_ref();
            let matched = display_name
                .into_iter()
                .chain(raw_name)
                .any(|name| tracked_names.contains(&normalize_author_name(name)));
            if matched {
                work.matched_author_names
                    .extend(display_name.into_iter().cloned());
                work.matched_author_names
                    .extend(raw_name.into_iter().cloned());
            }
        }
        work.matched_author_names.sort();
        work.matched_author_names.dedup();
    }
}

pub(crate) fn normalize_author_name(name: &str) -> String {
    let tokens = name
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();

    let mut significant = tokens
        .iter()
        .filter(|token| token.chars().count() > 1)
        .cloned()
        .collect::<Vec<_>>();
    if significant.is_empty() {
        significant = tokens;
    }

    significant.sort();
    significant.dedup();
    significant.join(" ")
}

fn normalize_doi(doi: &str) -> String {
    let doi = doi.trim().to_lowercase();
    let doi = doi
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("dx.")
        .trim_start_matches("doi.org/")
        .trim_start_matches("doi:");
    doi.trim().trim_end_matches('/').to_string()
}

fn normalize_title(title: &str) -> String {
    let mut normalized = String::new();
    let mut separated = false;

    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if separated && !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push(character);
            separated = false;
        } else if !normalized.is_empty() {
            separated = true;
        }
    }

    normalized
}

fn merge_work_version(existing: &mut Work, mut candidate: Work) {
    let provider_match = existing.provider_match || candidate.provider_match;
    let collection_date = existing
        .collection_date
        .take()
        .or_else(|| candidate.collection_date.take());
    let mut matched_author_names = std::mem::take(&mut existing.matched_author_names);
    matched_author_names.append(&mut candidate.matched_author_names);
    matched_author_names.sort();
    matched_author_names.dedup();
    let mut curated_sources = std::mem::take(&mut existing.curated_sources);
    curated_sources.append(&mut candidate.curated_sources);
    curated_sources.sort_by(|left, right| (&left.name, &left.url).cmp(&(&right.name, &right.url)));
    curated_sources.dedup();
    let mut curated_categories = std::mem::take(&mut existing.curated_categories);
    curated_categories.append(&mut candidate.curated_categories);
    curated_categories.sort();
    curated_categories.dedup();

    if version_quality(&candidate) > version_quality(existing) {
        std::mem::swap(existing, &mut candidate);
    }

    let selected_link = existing.best_link();
    let mut alternate_links = std::mem::take(&mut existing.alternate_links);
    alternate_links.append(&mut candidate.alternate_links);
    if let Some(link) = candidate.best_link() {
        alternate_links.push(link);
    }
    alternate_links.retain(|link| Some(link) != selected_link.as_ref());
    alternate_links.sort();
    alternate_links.dedup();
    existing.alternate_links = alternate_links;
    existing.collection_date = collection_date;
    existing.matched_author_names = matched_author_names;
    existing.provider_match = provider_match;
    existing.curated_sources = curated_sources;
    existing.curated_categories = curated_categories;
}

fn version_quality(work: &Work) -> (bool, bool, bool, &str) {
    let has_pdf = work.oa_pdf_url().is_some();
    let has_abstract = work
        .abstract_inverted_index
        .as_ref()
        .is_some_and(|index| !index.is_empty());
    let is_published = [&work.primary_location, &work.best_oa_location]
        .into_iter()
        .flatten()
        .any(|location| location.version.as_deref() == Some("publishedVersion"));
    let publication_date = work.publication_date.as_deref().unwrap_or("");

    (has_pdf, has_abstract, is_published, publication_date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openalex::{Author, Authorship};

    fn work_with_authors(names: &[&str]) -> Work {
        Work {
            id: Some("curated:1".to_string()),
            doi: None,
            title: Some("Curated paper".to_string()),
            display_name: None,
            publication_date: Some("2026-01-01".to_string()),
            collection_date: None,
            cited_by_count: None,
            authorships: Some(
                names
                    .iter()
                    .map(|name| Authorship {
                        author: Some(Author {
                            id: None,
                            display_name: Some((*name).to_string()),
                        }),
                        raw_author_name: Some((*name).to_string()),
                    })
                    .collect(),
            ),
            primary_location: None,
            best_oa_location: None,
            abstract_inverted_index: None,
            abstract_text_override: None,
            alternate_links: Vec::new(),
            matched_author_names: Vec::new(),
            provider_match: false,
            curated_sources: Vec::new(),
            curated_categories: Vec::new(),
        }
    }

    #[test]
    fn marks_configured_authors_on_records_without_provider_ids() {
        let mut works = vec![work_with_authors(&[
            "Baker, David",
            "Nicholas F Polizzi",
            "Untracked Author",
        ])];

        mark_authors_by_name(
            &mut works,
            &["David Baker".to_string(), "Nicholas F. Polizzi".to_string()],
        );

        assert_eq!(
            works[0].matched_author_names,
            vec!["Baker, David", "Nicholas F Polizzi"]
        );
    }

    #[test]
    fn configured_aliases_mark_the_corresponding_curated_author() {
        let mut works = vec![work_with_authors(&["David W Baker"])];

        mark_authors_by_name(&mut works, &["David W Baker".to_string()]);

        assert_eq!(works[0].matched_author_names, vec!["David W Baker"]);
    }

    #[test]
    fn does_not_fuzzily_match_different_authors() {
        let mut works = vec![work_with_authors(&["Daniel Baker"])];

        mark_authors_by_name(&mut works, &["David Baker".to_string()]);

        assert!(works[0].matched_author_names.is_empty());
    }

    #[test]
    fn merges_curated_arxiv_record_with_its_openalex_doi() {
        let enriched: Work = serde_json::from_value(serde_json::json!({
            "id": "https://openalex.org/W1",
            "doi": "https://doi.org/10.48550/arxiv.2605.26690",
            "title": "A title",
            "authorships": [{"author": {"display_name": "John Smith"}}]
        }))
        .unwrap();
        let curated: Work = serde_json::from_value(serde_json::json!({
            "id": "arxiv:2605.26690",
            "title": "A title",
            "authorships": [{"author": {"display_name": "J. Smith"}}]
        }))
        .unwrap();

        let merged = merge_works(vec![enriched], vec![curated]);

        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn preserves_collection_date_when_enriched_version_wins() {
        let enriched: Work = serde_json::from_value(serde_json::json!({
            "id": "https://openalex.org/W1",
            "doi": "https://doi.org/10.1000/example",
            "publication_date": "2026-01-02",
            "abstract_inverted_index": {"Abstract": [0]}
        }))
        .unwrap();
        let mut curated: Work = serde_json::from_value(serde_json::json!({
            "id": "doi:10.1000/example",
            "doi": "https://doi.org/10.1000/example"
        }))
        .unwrap();
        curated.collection_date = Some(crate::openalex::CollectionDate {
            date: "2025-05-03".to_string(),
            commit_url: "https://github.com/example/repo/commit/abc".to_string(),
        });

        let merged = merge_works(vec![enriched], vec![curated]);

        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0]
                .collection_date
                .as_ref()
                .map(|date| date.date.as_str()),
            Some("2025-05-03")
        );
    }

    #[test]
    fn merged_work_retains_provider_provenance() {
        let provider: Work = serde_json::from_value(serde_json::json!({
            "id": "https://openalex.org/W1",
            "doi": "https://doi.org/10.1000/example",
            "provider_match": true
        }))
        .unwrap();
        let curated: Work = serde_json::from_value(serde_json::json!({
            "id": "curated:1",
            "doi": "https://doi.org/10.1000/example",
            "curated_sources": [{
                "key": "collection",
                "name": "Collection",
                "url": "https://example.com/collection"
            }]
        }))
        .unwrap();

        let merged = merge_works(vec![provider], vec![curated]);

        assert_eq!(merged.len(), 1);
        assert!(merged[0].provider_match);
        assert_eq!(merged[0].curated_sources.len(), 1);
    }
}
