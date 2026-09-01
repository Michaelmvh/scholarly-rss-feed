//! Archived Google Scholar scraper provider.
//!
//! Google Scholar has no supported public API. This adapter preserves the
//! project's original HTML-scraping fallback while validating response status,
//! challenge pages, and required result structure before data enters the cache.

use crate::openalex::{Author, Authorship, Location, Source, Work};
use scraper::{ElementRef, Html, Selector};

const SCHOLAR_URL: &str = "https://scholar.google.com/scholar";
const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;

pub async fn fetch_works(
    client: &reqwest::Client,
    author_names: &[String],
    from: &str,
) -> Result<Vec<Work>, String> {
    let from_year = from
        .get(..4)
        .filter(|year| year.chars().all(|character| character.is_ascii_digit()));
    let mut works = Vec::new();

    for author_name in author_names {
        let mut params = vec![
            ("q", format!("author:\"{author_name}\"")),
            ("scisbd", "2".to_string()),
            ("num", "100".to_string()),
            ("hl", "en".to_string()),
            ("filter", "0".to_string()),
            ("as_vis", "1".to_string()),
        ];
        if let Some(year) = from_year {
            params.push(("as_ylo", year.to_string()));
        }
        let url = url::Url::parse_with_params(SCHOLAR_URL, &params)
            .map_err(|error| format!("Failed to build Google Scholar URL: {error}"))?;
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|error| format!("Google Scholar query for \"{author_name}\" failed: {error}"))?
            .error_for_status()
            .map_err(|error| {
                format!("Google Scholar query for \"{author_name}\" failed: {error}")
            })?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES)
        {
            return Err(format!(
                "Google Scholar response for \"{author_name}\" exceeded {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            format!("Failed to read Google Scholar response for \"{author_name}\": {error}")
        })? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES as usize {
                return Err(format!(
                    "Google Scholar response for \"{author_name}\" exceeded \
                     {MAX_RESPONSE_BYTES} bytes"
                ));
            }
            body.extend_from_slice(&chunk);
        }
        let html = String::from_utf8(body).map_err(|error| {
            format!("Google Scholar response for \"{author_name}\" was not UTF-8: {error}")
        })?;
        let results = parse_results(&html).map_err(|error| {
            format!("Failed to parse Google Scholar results for \"{author_name}\": {error}")
        })?;

        works.extend(
            results
                .into_iter()
                .map(|result| result_to_work(result, author_name)),
        );
    }

    Ok(works)
}

#[derive(Debug)]
struct ScholarResult {
    title: String,
    authors: String,
    abstract_text: String,
    venue: Option<String>,
    link: String,
    pdf_link: Option<String>,
    year: Option<String>,
    citations: Option<u64>,
}

fn parse_results(html: &str) -> Result<Vec<ScholarResult>, String> {
    let lowercase = html.to_ascii_lowercase();
    if lowercase.contains("unusual traffic")
        || lowercase.contains("not a robot")
        || lowercase.contains("recaptcha")
    {
        return Err("Google Scholar returned a bot challenge".to_string());
    }

    let document = Html::parse_document(html);
    let result_selector = selector(".gs_r.gs_or");
    let result_nodes = document.select(&result_selector).collect::<Vec<_>>();
    if result_nodes.is_empty() {
        if lowercase.contains("did not match any articles") {
            return Ok(Vec::new());
        }
        return Err("response did not contain a recognizable results page".to_string());
    }

    result_nodes.into_iter().map(parse_result).collect()
}

fn parse_result(result: ElementRef<'_>) -> Result<ScholarResult, String> {
    let title_selector = selector(".gs_rt");
    let title_link_selector = selector(".gs_rt a");
    let authors_selector = selector(".gs_a");
    let abstract_selector = selector(".gs_rs");
    let pdf_selector = selector(".gs_or_ggsm a");
    let actions_selector = selector(".gs_fl a");

    let title_element = result
        .select(&title_selector)
        .next()
        .ok_or_else(|| "result is missing its title".to_string())?;
    let title = element_text(title_element);
    if title.is_empty() {
        return Err("result has an empty title".to_string());
    }
    let title_link = result
        .select(&title_link_selector)
        .next()
        .ok_or_else(|| format!("result \"{title}\" is missing its article link"))?;
    let link = title_link
        .value()
        .attr("href")
        .filter(|link| !link.trim().is_empty())
        .ok_or_else(|| format!("result \"{title}\" has an empty article link"))?
        .to_string();
    let metadata = result
        .select(&authors_selector)
        .next()
        .map(element_text)
        .unwrap_or_default();
    let mut metadata_parts = metadata.split(" - ").map(str::trim);
    let authors = metadata_parts.next().unwrap_or_default().to_string();
    let venue_metadata = metadata_parts.next().filter(|value| !value.is_empty());
    let year = find_year(&metadata);
    let venue = venue_metadata
        .map(|value| {
            year.as_deref()
                .map(|year| value.replace(year, "").trim_matches([' ', ',']).to_string())
                .unwrap_or_else(|| value.to_string())
        })
        .filter(|value| !value.is_empty());
    let abstract_text = result
        .select(&abstract_selector)
        .next()
        .map(element_text)
        .unwrap_or_default();
    let pdf_link = result
        .select(&pdf_selector)
        .next()
        .and_then(|element| element.value().attr("href"))
        .map(str::to_string);
    let citations = result
        .select(&actions_selector)
        .map(element_text)
        .find_map(|text| {
            text.strip_prefix("Cited by ")
                .and_then(|count| count.parse::<u64>().ok())
        });

    Ok(ScholarResult {
        title,
        authors,
        abstract_text,
        venue,
        link,
        pdf_link,
        year,
        citations,
    })
}

fn result_to_work(result: ScholarResult, queried_author: &str) -> Work {
    let author_names = split_authors(&result.authors);
    let matched_author_names = best_author_match(&author_names, queried_author)
        .into_iter()
        .cloned()
        .collect();
    let authorships = author_names
        .into_iter()
        .map(|name| Authorship {
            author: Some(Author {
                id: None,
                display_name: Some(name.clone()),
            }),
            raw_author_name: Some(name),
        })
        .collect();
    let publication_date = result.year.map(|year| format!("{year}-01-01"));
    let source_name = result.venue.or_else(|| {
        url::Url::parse(&result.link)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
    });
    let primary_location = Some(Location {
        landing_page_url: Some(result.link.clone()),
        pdf_url: None,
        source: source_name.map(|display_name| Source {
            display_name: Some(display_name),
        }),
        version: None,
    });
    let best_oa_location = result.pdf_link.map(|pdf_url| Location {
        landing_page_url: None,
        pdf_url: Some(pdf_url),
        source: None,
        version: None,
    });

    Work {
        id: Some(result.link.clone()),
        doi: None,
        title: Some(result.title),
        display_name: None,
        publication_date,
        latest_version_date: None,
        collection_date: None,
        cited_by_count: result.citations,
        authorships: Some(authorships),
        primary_location,
        best_oa_location,
        abstract_inverted_index: None,
        abstract_text_override: (!result.abstract_text.trim().is_empty())
            .then_some(result.abstract_text),
        alternate_links: Vec::new(),
        matched_author_names,
        discovery_sources: Vec::new(),
        curated_categories: Vec::new(),
    }
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static Google Scholar selector must be valid")
}

fn element_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_year(value: &str) -> Option<String> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| {
            part.len() == 4
                && part
                    .parse::<u16>()
                    .is_ok_and(|year| (1000..=2999).contains(&year))
        })
        .map(str::to_string)
}

fn split_authors(authors: &str) -> Vec<String> {
    authors
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != "…")
        .map(str::to_string)
        .collect()
}

fn best_author_match<'a>(authors: &'a [String], queried_author: &str) -> Option<&'a String> {
    authors
        .iter()
        .map(|author| (author_match_score(author, queried_author), author))
        .filter(|(score, _)| *score > 0)
        .max_by_key(|(score, _)| *score)
        .map(|(_, author)| author)
}

fn author_match_score(candidate: &str, queried_author: &str) -> u8 {
    let candidate_parts = name_parts(candidate);
    let queried_parts = name_parts(queried_author);
    let (Some(candidate_surname), Some(queried_surname)) =
        (candidate_parts.last(), queried_parts.last())
    else {
        return 0;
    };
    if candidate_surname != queried_surname {
        return 0;
    }

    let (Some(candidate_given), Some(queried_given)) =
        (candidate_parts.first(), queried_parts.first())
    else {
        return 0;
    };
    if candidate_given == queried_given {
        3
    } else if candidate_given.starts_with(queried_given)
        || queried_given.starts_with(candidate_given)
    {
        2
    } else if candidate_given.chars().next() == queried_given.chars().next() {
        1
    } else {
        0
    }
}

fn name_parts(name: &str) -> Vec<String> {
    name.split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESULT_HTML: &str = r#"
      <html><body><div id="gs_res_ccl_mid">
        <div class="gs_r gs_or gs_scl">
          <div class="gs_or_ggsm"><a href="https://example.com/article.pdf">PDF</a></div>
          <h3 class="gs_rt"><a href="https://example.com/article">An archived result</a></h3>
          <div class="gs_a">P Chatterjee, A Lovelace - Example Conference, 2025 - example.com</div>
          <div class="gs_rs">A useful <b>abstract</b>.</div>
          <div class="gs_fl"><a>Cited by 42</a></div>
        </div>
      </div></body></html>
    "#;

    #[test]
    fn parses_and_converts_archived_results() {
        let results = parse_results(RESULT_HTML).unwrap();
        let work = result_to_work(results.into_iter().next().unwrap(), "Pranam Chatterjee");

        assert_eq!(work.publication_date.as_deref(), Some("2025-01-01"));
        assert_eq!(work.author_names(), vec!["P Chatterjee", "A Lovelace"]);
        assert_eq!(work.matched_author_names, vec!["P Chatterjee"]);
        assert_eq!(work.abstract_text().as_deref(), Some("A useful abstract ."));
        assert_eq!(
            work.oa_pdf_url().as_deref(),
            Some("https://example.com/article.pdf")
        );
    }

    #[test]
    fn rejects_challenge_and_unrecognized_pages() {
        assert!(parse_results("<html>Our systems detected unusual traffic</html>").is_err());
        assert!(parse_results("<html><body>Sign in</body></html>").is_err());
    }

    #[test]
    fn accepts_a_recognizable_empty_results_page() {
        assert!(
            parse_results("<div id=\"gs_res_ccl_mid\">did not match any articles</div>")
                .unwrap()
                .is_empty()
        );
        assert!(parse_results("<div id=\"gs_res_ccl_mid\"></div>").is_err());
    }

    #[test]
    fn author_matching_does_not_mark_same_surname_with_different_initial() {
        let authors = vec!["D Baker".to_string(), "M Baker".to_string()];

        assert_eq!(
            best_author_match(&authors, "David Baker").map(String::as_str),
            Some("D Baker")
        );
    }
}
