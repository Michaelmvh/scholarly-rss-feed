use crate::openalex::CollectionDate;
use hyper::StatusCode;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

const PELDOM_BLAME_URL: &str =
    "https://github.com/Peldom/papers_for_protein_design_using_DL/blame/main/README.md";
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub async fn fetch_collection_dates(
    client: &reqwest::Client,
    titles: &HashSet<String>,
) -> Result<HashMap<String, CollectionDate>, String> {
    if titles.is_empty() {
        return Ok(HashMap::new());
    }
    let response = client
        .get(PELDOM_BLAME_URL)
        .send()
        .await
        .map_err(|error| format!("GitHub blame request failed: {error}"))?;
    if response.status() != StatusCode::OK {
        return Err(format!(
            "GitHub blame request returned HTTP {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(format!(
            "GitHub blame response exceeded the {MAX_RESPONSE_BYTES}-byte limit"
        ));
    }

    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Failed reading GitHub blame response: {error}"))?
    {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(format!(
                "GitHub blame response exceeded the {MAX_RESPONSE_BYTES}-byte limit"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let html = String::from_utf8(body)
        .map_err(|error| format!("GitHub blame response was not valid UTF-8: {error}"))?;
    parse_collection_dates(&html, titles)
}

fn parse_collection_dates(
    html: &str,
    titles: &HashSet<String>,
) -> Result<HashMap<String, CollectionDate>, String> {
    let selector = Selector::parse(r#"script[type="application/json"]"#)
        .map_err(|error| format!("Failed to build GitHub blame selector: {error}"))?;
    let document = Html::parse_document(html);
    let embedded = document
        .select(&selector)
        .filter_map(|element| serde_json::from_str::<EmbeddedData>(&element.inner_html()).ok())
        .find(|data| data.payload.blame_route.is_some())
        .ok_or_else(|| "GitHub blame response did not contain blame data".to_string())?;
    let styled_blob = embedded
        .payload
        .styled_blob
        .ok_or_else(|| "GitHub blame response did not contain file lines".to_string())?;
    let blame = embedded
        .payload
        .blame_route
        .expect("blame route was checked")
        .blame;
    let ranges = blame.ranges.into_values().collect::<Vec<_>>();
    let mut dates = HashMap::new();

    for (index, line) in styled_blob.raw_lines.iter().enumerate() {
        let Some(title) = markdown_title(line) else {
            continue;
        };
        if !titles.contains(title) {
            continue;
        }
        let line_number = index + 1;
        let Some(range) = ranges
            .iter()
            .find(|range| range.start <= line_number && line_number <= range.end)
        else {
            continue;
        };
        let Some(commit) = blame.commits.get(&range.commit_oid) else {
            continue;
        };
        let Some(date) = commit.committed_date.get(..10) else {
            continue;
        };
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
            continue;
        }
        dates.insert(
            title.to_string(),
            CollectionDate {
                date: date.to_string(),
                commit_url: format!(
                    "https://github.com/Peldom/papers_for_protein_design_using_DL/commit/{}",
                    range.commit_oid
                ),
            },
        );
    }
    Ok(dates)
}

fn markdown_title(line: &str) -> Option<&str> {
    let content = line.trim().strip_prefix("**")?;
    let end = content.find("**")?;
    let title = content[..end].trim();
    (!title.is_empty()).then_some(title)
}

#[derive(Deserialize)]
struct EmbeddedData {
    payload: Payload,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "codeViewBlameRoute")]
    blame_route: Option<BlameRoute>,
    #[serde(rename = "codeViewBlobLayoutRoute.StyledBlob")]
    styled_blob: Option<StyledBlob>,
}

#[derive(Deserialize)]
struct BlameRoute {
    blame: BlameData,
}

#[derive(Deserialize)]
struct BlameData {
    ranges: HashMap<String, BlameRange>,
    commits: HashMap<String, BlameCommit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlameRange {
    start: usize,
    end: usize,
    commit_oid: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlameCommit {
    committed_date: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StyledBlob {
    raw_lines: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_collection_date_from_embedded_blame_data() {
        let html = r#"
<script type="application/json">
{"payload":{
  "codeViewBlameRoute":{"blame":{
    "ranges":{"1":{"start":1,"end":2,"commitOid":"abc123"}},
    "commits":{"abc123":{"committedDate":"2025-05-03T14:38:14.000+08:00"}}
  }},
  "codeViewBlobLayoutRoute.StyledBlob":{"rawLines":[
    "**An undated paper**",
    "Example Author"
  ]}
}}
</script>
"#;
        let titles = HashSet::from(["An undated paper".to_string()]);

        let dates = parse_collection_dates(html, &titles).unwrap();

        assert_eq!(dates["An undated paper"].date, "2025-05-03");
        assert!(dates["An undated paper"].commit_url.ends_with("/abc123"));
    }

    #[test]
    fn rejects_pages_without_blame_data() {
        assert!(parse_collection_dates("<html></html>", &HashSet::new()).is_err());
    }
}
