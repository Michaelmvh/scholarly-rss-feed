use crate::openalex::{Author, Authorship, CuratedSource, Location, Source, Work};
use hyper::header::{ETAG, IF_NONE_MATCH};
use hyper::StatusCode;
use lazy_static::lazy_static;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const PELDOM_PROTEIN_DESIGN: &str = "peldom-protein-design";

const PELDOM_README_URL: &str =
    "https://raw.githubusercontent.com/Peldom/papers_for_protein_design_using_DL/main/README.md";
const PELDOM_REPOSITORY_URL: &str = "https://github.com/Peldom/papers_for_protein_design_using_DL";
const PELDOM_SOURCE_NAME: &str = "Papers for Protein Design Using Deep Learning";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MIN_EXPECTED_PAPERS: usize = 100;
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

lazy_static! {
    static ref PELDOM_SNAPSHOT: Arc<RwLock<Option<Snapshot>>> = Arc::new(RwLock::new(None));
    static ref PELDOM_REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::new(());
}

#[derive(Clone)]
struct Snapshot {
    etag: Option<String>,
    checked_at: Instant,
    works: Vec<Work>,
}

pub fn validate_sources(source_names: &[String]) -> Result<(), String> {
    for source_name in source_names {
        if source_name != PELDOM_PROTEIN_DESIGN {
            return Err(format!("Unknown curated source \"{source_name}\"."));
        }
    }
    Ok(())
}

pub async fn fetch_sources(
    client: &reqwest::Client,
    source_names: &[String],
    from: &str,
) -> Result<Vec<Work>, String> {
    validate_sources(source_names)?;
    let mut works = Vec::new();

    for source_name in source_names {
        match source_name.as_str() {
            PELDOM_PROTEIN_DESIGN => {
                works.extend(fetch_peldom(client).await?);
            }
            _ => unreachable!("curated sources were validated"),
        }
    }

    filter_from(&mut works, from);

    Ok(works)
}

fn filter_from(works: &mut Vec<Work>, from: &str) {
    let Some(from_year) = from.get(..4).and_then(|year| year.parse::<u16>().ok()) else {
        return;
    };
    works.retain(|work| {
        match work
            .publication_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .and_then(|year| year.parse::<u16>().ok())
        {
            Some(year) => year >= from_year,
            None => from_year <= 1900,
        }
    });
}

async fn fetch_peldom(client: &reqwest::Client) -> Result<Vec<Work>, String> {
    if let Some(snapshot) = fresh_snapshot() {
        return Ok(snapshot.works);
    }

    let _refresh_guard = PELDOM_REFRESH.lock().await;
    if let Some(snapshot) = fresh_snapshot() {
        return Ok(snapshot.works);
    }

    let cached = PELDOM_SNAPSHOT.read().clone();
    match refresh_peldom(client, cached.as_ref()).await {
        Ok(snapshot) => {
            let works = snapshot.works.clone();
            *PELDOM_SNAPSHOT.write() = Some(snapshot);
            Ok(works)
        }
        Err(error) => match cached {
            Some(mut snapshot) => {
                eprintln!("{error}; using the last successfully parsed Peldom snapshot");
                snapshot.checked_at = Instant::now();
                let works = snapshot.works.clone();
                *PELDOM_SNAPSHOT.write() = Some(snapshot);
                Ok(works)
            }
            None => Err(error),
        },
    }
}

fn fresh_snapshot() -> Option<Snapshot> {
    PELDOM_SNAPSHOT
        .read()
        .as_ref()
        .filter(|snapshot| snapshot.checked_at.elapsed() < REFRESH_INTERVAL)
        .cloned()
}

async fn refresh_peldom(
    client: &reqwest::Client,
    cached: Option<&Snapshot>,
) -> Result<Snapshot, String> {
    let mut request = client.get(PELDOM_README_URL);
    if let Some(etag) = cached.and_then(|snapshot| snapshot.etag.as_deref()) {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let mut response = request
        .send()
        .await
        .map_err(|error| format!("Peldom source request failed: {error}"))?;

    if response.status() == StatusCode::NOT_MODIFIED {
        let mut snapshot = cached
            .cloned()
            .ok_or_else(|| "Peldom returned 304 without a cached snapshot".to_string())?;
        snapshot.checked_at = Instant::now();
        return Ok(snapshot);
    }
    response = response
        .error_for_status()
        .map_err(|error| format!("Peldom source request failed: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(format!(
            "Peldom source exceeded the {MAX_RESPONSE_BYTES}-byte limit"
        ));
    }

    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Failed reading Peldom source: {error}"))?
    {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(format!(
                "Peldom source exceeded the {MAX_RESPONSE_BYTES}-byte limit"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let markdown = String::from_utf8(body)
        .map_err(|error| format!("Peldom source was not valid UTF-8: {error}"))?;
    let works = parse_peldom(&markdown)?;
    if works.len() < MIN_EXPECTED_PAPERS {
        return Err(format!(
            "Peldom parser found only {} stable-ID papers; expected at least {MIN_EXPECTED_PAPERS}",
            works.len()
        ));
    }

    Ok(Snapshot {
        etag,
        checked_at: Instant::now(),
        works,
    })
}

fn parse_peldom(markdown: &str) -> Result<Vec<Work>, String> {
    let mut section: Option<String> = None;
    let mut subsection: Option<String> = None;
    let mut pending: Option<PendingPaper> = None;
    let mut works = Vec::new();

    for line in markdown.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            finish_pending(&mut pending, &mut works);
            section = Some(heading.trim().to_string());
            subsection = None;
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("### ") {
            finish_pending(&mut pending, &mut works);
            subsection = Some(heading.trim().to_string());
            continue;
        }
        if let Some(title) = paper_title(trimmed) {
            finish_pending(&mut pending, &mut works);
            if included_section(section.as_deref(), subsection.as_deref()) {
                pending = Some(PendingPaper {
                    title,
                    section: section.clone().unwrap_or_default(),
                    subsection: subsection.clone(),
                    lines: Vec::new(),
                });
            }
            continue;
        }
        if let Some(paper) = &mut pending {
            paper.lines.push(trimmed.to_string());
        }
    }
    finish_pending(&mut pending, &mut works);

    if works.is_empty() {
        return Err("Peldom document contained no recognizable stable-ID papers".to_string());
    }
    Ok(works)
}

struct PendingPaper {
    title: String,
    section: String,
    subsection: Option<String>,
    lines: Vec<String>,
}

fn finish_pending(pending: &mut Option<PendingPaper>, works: &mut Vec<Work>) {
    let Some(paper) = pending.take() else {
        return;
    };
    if paper
        .lines
        .iter()
        .any(|line| line.to_ascii_lowercase().contains("paper unavailable"))
    {
        return;
    }
    if let Some(work) = paper.into_work() {
        works.push(work);
    }
}

impl PendingPaper {
    fn into_work(self) -> Option<Work> {
        let links = self
            .lines
            .iter()
            .flat_map(|line| markdown_links(line))
            .collect::<Vec<_>>();
        let (identifier, paper_link, venue) = stable_identifier(&links)?;
        let authors = self
            .lines
            .iter()
            .find(|line| {
                !line.is_empty()
                    && !line.starts_with('[')
                    && !line.starts_with('+')
                    && !line.starts_with('<')
            })
            .map(|line| split_authors(line))
            .unwrap_or_default();
        let publication_date = self
            .lines
            .iter()
            .find_map(|line| find_year(line))
            .map(|year| format!("{year}-01-01"));
        let source = venue.clone().map(|display_name| Source {
            display_name: Some(display_name),
        });
        let categories = [Some(self.section), self.subsection]
            .into_iter()
            .flatten()
            .collect();

        Some(Work {
            id: Some(identifier.clone()),
            doi: identifier
                .strip_prefix("doi:")
                .map(|doi| format!("https://doi.org/{doi}")),
            title: Some(self.title),
            display_name: None,
            publication_date,
            cited_by_count: None,
            authorships: Some(
                authors
                    .into_iter()
                    .map(|name| Authorship {
                        author: Some(Author {
                            id: None,
                            display_name: Some(name.clone()),
                        }),
                        raw_author_name: Some(name),
                    })
                    .collect(),
            ),
            primary_location: Some(Location {
                landing_page_url: Some(paper_link),
                pdf_url: None,
                source,
                version: None,
            }),
            best_oa_location: None,
            abstract_inverted_index: None,
            abstract_text_override: None,
            alternate_links: Vec::new(),
            matched_author_names: Vec::new(),
            curated_sources: vec![CuratedSource {
                name: PELDOM_SOURCE_NAME.to_string(),
                url: PELDOM_REPOSITORY_URL.to_string(),
            }],
            curated_categories: categories,
        })
    }
}

fn paper_title(line: &str) -> Option<String> {
    let content = line.strip_prefix("**")?;
    let end = content.find("**")?;
    let title = content[..end].trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn included_section(section: Option<&str>, subsection: Option<&str>) -> bool {
    let section_number = section
        .and_then(|heading| heading.split_once('.'))
        .and_then(|(number, _)| number.trim().parse::<u8>().ok());
    if !matches!(section_number, Some(1..=7)) {
        return false;
    }
    !subsection.is_some_and(|heading| heading.starts_with("7.3") || heading.starts_with("7.5"))
}

fn markdown_links(line: &str) -> Vec<(String, String)> {
    let mut links = Vec::new();
    let mut remaining = line;
    while let Some(label_start) = remaining.find('[') {
        remaining = &remaining[label_start + 1..];
        let Some(label_end) = remaining.find("](") else {
            continue;
        };
        let label = remaining[..label_end].trim_matches(['[', ']']).trim();
        remaining = &remaining[label_end + 2..];
        let Some(url_end) = remaining.find(')') else {
            break;
        };
        let url = remaining[..url_end].trim();
        if url.starts_with("http://") || url.starts_with("https://") {
            links.push((label.to_string(), url.to_string()));
        }
        remaining = &remaining[url_end + 1..];
    }
    links
}

fn stable_identifier(links: &[(String, String)]) -> Option<(String, String, Option<String>)> {
    for (label, url) in links {
        if is_artifact_link(label, url) {
            continue;
        }
        if let Some(doi) = extract_doi(url) {
            return Some((
                format!("doi:{doi}"),
                format!("https://doi.org/{doi}"),
                Some(label.clone()),
            ));
        }
    }
    for (label, url) in links {
        if is_artifact_link(label, url) {
            continue;
        }
        if let Some(arxiv_id) = extract_arxiv_id(url) {
            return Some((
                format!("arxiv:{arxiv_id}"),
                url.clone(),
                Some(label.clone()),
            ));
        }
    }
    None
}

fn is_artifact_link(label: &str, url: &str) -> bool {
    let label = label.to_ascii_lowercase();
    let url = url.to_ascii_lowercase();
    [
        "code",
        "github",
        "model",
        "dataset",
        "supplement",
        "website",
        "project",
        "checkpoint",
        "lecture",
        "video",
    ]
    .iter()
    .any(|marker| label.contains(marker))
        || url.contains("doi.org/10.5281/zenodo.")
}

fn extract_doi(url: &str) -> Option<String> {
    let lowercase = url.to_ascii_lowercase();
    let supported_context = lowercase.contains("doi.org/10.")
        || lowercase.contains("/doi/10.")
        || lowercase.contains("biorxiv.org/content/10.");
    if !supported_context {
        return None;
    }
    let start = lowercase.find("10.")?;
    let mut doi = lowercase[start..]
        .split(['?', '#', ')', ']', ' '])
        .next()?
        .trim_end_matches('/')
        .to_string();
    if doi.starts_with("10.1101/") || doi.starts_with("10.64898/") {
        doi = strip_preprint_version(&doi);
    } else if !lowercase.contains("doi.org/") {
        doi = strip_publisher_route_suffix(&doi);
    }
    let (prefix, suffix) = doi.split_once('/')?;
    let valid_prefix = prefix
        .strip_prefix("10.")
        .is_some_and(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()));
    (valid_prefix && !suffix.is_empty()).then_some(doi)
}

fn strip_publisher_route_suffix(doi: &str) -> String {
    let Some((prefix, suffix)) = doi.rsplit_once('/') else {
        return doi.to_string();
    };
    if suffix.chars().all(|character| character.is_ascii_digit())
        && prefix.matches('/').count() >= 2
    {
        prefix.to_string()
    } else {
        doi.to_string()
    }
}

fn strip_preprint_version(doi: &str) -> String {
    let Some(version_start) = doi.rfind('v') else {
        return doi.to_string();
    };
    let suffix = &doi[version_start + 1..];
    let digit_count = suffix
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .count();
    if digit_count > 0 && (digit_count == suffix.len() || suffix[digit_count..].starts_with('.')) {
        doi[..version_start].to_string()
    } else {
        doi.to_string()
    }
}

fn extract_arxiv_id(url: &str) -> Option<String> {
    let lowercase = url.to_ascii_lowercase();
    let start = lowercase.find("arxiv.org/abs/")? + "arxiv.org/abs/".len();
    let id = lowercase[start..]
        .split(['?', '#', '/', ')', ']', ' '])
        .next()?;
    let id = strip_preprint_version(id);
    (!id.is_empty()
        && id
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.'))
    .then_some(id)
}

fn split_authors(line: &str) -> Vec<String> {
    let citation_style = line.contains(", and ");
    let mut authors = line
        .replace(", and ", ", ")
        .replace(" and ", ", ")
        .replace(" & ", ", ")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let repeated_citation_pairs = authors.len() >= 4
        && authors.len().is_multiple_of(2)
        && authors
            .chunks_exact(2)
            .all(|pair| !pair[0].contains(' ') && looks_like_initials(&pair[1]));
    if repeated_citation_pairs {
        return authors
            .chunks_exact(2)
            .map(|pair| format!("{} {}", pair[1], pair[0]))
            .collect();
    }
    if citation_style
        && authors.len() >= 3
        && authors[0].split_whitespace().count() == 1
        && authors[1].split_whitespace().count() == 1
    {
        let given_name = authors.remove(1);
        authors[0] = format!("{given_name} {}", authors[0]);
    }
    authors
}

fn looks_like_initials(name: &str) -> bool {
    let letter_count = name
        .chars()
        .filter(|character| character.is_alphabetic())
        .count();
    (1..=3).contains(&letter_count)
        && name
            .chars()
            .all(|character| character.is_alphabetic() || ".- ".contains(character))
}

fn find_year(line: &str) -> Option<u16> {
    line.split(|character: char| !character.is_ascii_digit())
        .find_map(|part| {
            (part.len() == 4)
                .then(|| part.parse::<u16>().ok())
                .flatten()
                .filter(|year| (1900..=2999).contains(year))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
# List

## 0. Benchmarks and datasets

### 0.1 Sequence

**Excluded benchmark**
Ada Lovelace
[bioRxiv 2025](https://www.biorxiv.org/content/10.1101/2025.01.01.123456v1)

## 3. Function to Scaffold

### 3.4 Diffusion-based

**A DOI paper**
Ada Lovelace, Grace Hopper
[bioRxiv 2026.05.06.723381](https://www.biorxiv.org/content/10.64898/2026.05.06.723381v2) • [code](https://github.com/example/code)

**An arXiv paper**
Alan Turing and Joan Clarke
[arXiv:2602.04916](https://arxiv.org/abs/2602.04916v3)

**An undated DOI paper**
Syrlybaeva, Raulia, and Eva-Maria Strauch
[Science](https://www.science.org/doi/10.1126/science.aee1792)

**An artifact-only record**
Example Author
[code](https://doi.org/10.5281/zenodo.17089342)

**Unavailable paper**
Example Author
Paper unavailable at [Workshop 2026](https://example.com/workshop)

## 7. Other

### 7.3 Molecular Design Models

**Excluded molecule paper**
Example Author
[arXiv](https://arxiv.org/abs/2601.00001)
"#;

    #[test]
    fn parses_only_stable_ids_from_included_sections() {
        let works = parse_peldom(FIXTURE).unwrap();

        assert_eq!(works.len(), 3);
        assert_eq!(
            works[0].id.as_deref(),
            Some("doi:10.64898/2026.05.06.723381")
        );
        assert_eq!(
            works[0].author_names(),
            vec!["Ada Lovelace", "Grace Hopper"]
        );
        assert_eq!(
            works[0].curated_categories,
            vec!["3. Function to Scaffold", "3.4 Diffusion-based"]
        );
        assert_eq!(works[1].id.as_deref(), Some("arxiv:2602.04916"));
        assert_eq!(works[1].author_names(), vec!["Alan Turing", "Joan Clarke"]);
        assert_eq!(works[2].id.as_deref(), Some("doi:10.1126/science.aee1792"));
        assert_eq!(
            works[2].author_names(),
            vec!["Raulia Syrlybaeva", "Eva-Maria Strauch"]
        );
        assert!(works[2].publication_date.is_none());
    }

    #[test]
    fn extracts_dois_embedded_in_publisher_paths() {
        assert_eq!(
            extract_doi("https://academic.oup.com/article/doi/10.1093/bioinformatics/btad027/1")
                .as_deref(),
            Some("10.1093/bioinformatics/btad027")
        );
        assert!(extract_doi("https://nature.com/articles/s41592-019-0496-6").is_none());
        assert_eq!(
            extract_doi("https://www.biorxiv.org/content/10.64898/2026.05.09.722041v1.full.pdf")
                .as_deref(),
            Some("10.64898/2026.05.09.722041")
        );
    }

    #[test]
    fn rejects_documents_without_recognizable_papers() {
        assert!(parse_peldom("# unexpected content").is_err());
    }

    #[test]
    fn validates_registered_source_names() {
        assert!(validate_sources(&[PELDOM_PROTEIN_DESIGN.to_string()]).is_ok());
        assert!(validate_sources(&["unknown".to_string()]).is_err());
    }

    #[test]
    fn full_archive_keeps_undated_stable_id_papers() {
        let mut works = parse_peldom(FIXTURE).unwrap();

        filter_from(&mut works, "1900-01-01");
        assert!(works.iter().any(|work| work.publication_date.is_none()));

        filter_from(&mut works, "2025-01-01");
        assert!(works.iter().all(|work| work.publication_date.is_some()));
    }

    #[test]
    fn parses_repeated_surname_initial_citations_as_people() {
        assert_eq!(
            split_authors("Notin, P., Dias, M., Meier, J., Gal, Y"),
            vec!["P. Notin", "M. Dias", "J. Meier", "Y Gal"]
        );
    }

    #[test]
    fn separates_ampersand_delimited_authors() {
        assert_eq!(
            split_authors("Gaetano T. Montelione & David Baker"),
            vec!["Gaetano T. Montelione", "David Baker"]
        );
    }
}
