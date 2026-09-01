use crate::curated::SourceDiagnostics;
use crate::openalex::Work;
use crate::works::{merge_works, version_keys, VersionKey};
use std::collections::BTreeMap;

const MAX_DISCOVERIES: usize = 20;

#[derive(Debug, PartialEq, Eq)]
pub struct ComparisonReport {
    provider_count: usize,
    curated_entry_count: usize,
    curated_paper_count: usize,
    curated_duplicate_count: usize,
    overlap_count: usize,
    doi_overlap_count: usize,
    title_author_overlap_count: usize,
    unique_curated_count: usize,
    merged_count: usize,
    curated_missing_date: usize,
    curated_missing_abstract: usize,
    curated_missing_pdf: usize,
    curated_missing_venue: usize,
    curated_missing_authors: usize,
    overlap_abstract_enrichments: usize,
    overlap_pdf_enrichments: usize,
    categories: BTreeMap<String, usize>,
    discoveries: Vec<String>,
    diagnostics: Vec<SourceDiagnostics>,
}

pub fn compare(
    provider_works: &[Work],
    curated_works: &[Work],
    diagnostics: Vec<SourceDiagnostics>,
) -> ComparisonReport {
    let recent_entry_count = curated_works.len();
    let curated_works = merge_works(Vec::new(), curated_works.to_vec());
    let mut overlap_count = 0;
    let mut doi_overlap_count = 0;
    let mut title_author_overlap_count = 0;
    let mut overlap_abstract_enrichments = 0;
    let mut overlap_pdf_enrichments = 0;
    let mut discoveries = Vec::new();

    for curated in &curated_works {
        let matched = provider_works
            .iter()
            .filter_map(|provider| match_kind(provider, curated).map(|kind| (provider, kind)))
            .next();
        if let Some((provider, kind)) = matched {
            overlap_count += 1;
            match kind {
                MatchKind::Doi => doi_overlap_count += 1,
                MatchKind::TitleAuthors => title_author_overlap_count += 1,
            }
            if curated.abstract_text().is_none() && provider.abstract_text().is_some() {
                overlap_abstract_enrichments += 1;
            }
            if curated.oa_pdf_url().is_none() && provider.oa_pdf_url().is_some() {
                overlap_pdf_enrichments += 1;
            }
        } else if discoveries.len() < MAX_DISCOVERIES {
            let date = curated.publication_date.as_deref().unwrap_or("undated");
            discoveries.push(format!("{date} — {}", curated.best_title()));
        }
    }

    let mut categories = BTreeMap::new();
    for work in &curated_works {
        if let Some(category) = work.curated_categories.last() {
            *categories.entry(category.clone()).or_insert(0) += 1;
        }
    }

    ComparisonReport {
        provider_count: provider_works.len(),
        curated_entry_count: diagnostics
            .iter()
            .map(|diagnostic| diagnostic.accepted)
            .sum(),
        curated_paper_count: curated_works.len(),
        curated_duplicate_count: recent_entry_count.saturating_sub(curated_works.len()),
        overlap_count,
        doi_overlap_count,
        title_author_overlap_count,
        unique_curated_count: curated_works.len().saturating_sub(overlap_count),
        merged_count: merge_works(provider_works.to_vec(), curated_works.clone()).len(),
        curated_missing_date: curated_works
            .iter()
            .filter(|work| work.publication_date.is_none())
            .count(),
        curated_missing_abstract: curated_works
            .iter()
            .filter(|work| work.abstract_text().is_none())
            .count(),
        curated_missing_pdf: curated_works
            .iter()
            .filter(|work| work.oa_pdf_url().is_none())
            .count(),
        curated_missing_venue: curated_works
            .iter()
            .filter(|work| work.venue().is_none())
            .count(),
        curated_missing_authors: curated_works
            .iter()
            .filter(|work| work.author_names().is_empty())
            .count(),
        overlap_abstract_enrichments,
        overlap_pdf_enrichments,
        categories,
        discoveries,
        diagnostics,
    }
}

impl ComparisonReport {
    pub fn render_markdown(
        &self,
        feed_name: &str,
        source_name: &str,
        from: &str,
        provider_name: &str,
    ) -> String {
        let mut output = format!(
            "# Curated source comparison\n\n\
             - Feed: `{feed_name}`\n\
             - Provider: {provider_name}\n\
             - Curated source: `{source_name}`\n\
             - Cutoff: `{from}`\n\n\
             ## Coverage\n\n\
             | Metric | Count |\n\
             |---|---:|\n\
             | Provider papers | {} |\n\
             | Accepted curated entries (all time) | {} |\n\
             | Distinct curated papers within cutoff | {} |\n\
             | Duplicate/version entries within cutoff | {} |\n\
             | Overlapping papers | {} |\n\
             | Overlap matched by DOI | {} |\n\
             | Overlap matched by title and authors | {} |\n\
             | Unique curated discoveries | {} |\n\
             | Papers after version merging | {} |\n\n\
             ## Curated metadata quality\n\n\
             | Missing field | Count |\n\
             |---|---:|\n\
             | Publication date | {} |\n\
             | Abstract | {} |\n\
             | Open-access PDF | {} |\n\
             | Venue | {} |\n\
             | Authors | {} |\n\n\
             Provider records could add abstracts to {} overlapping papers and open-access \
             PDFs to {} overlapping papers.\n\n",
            self.provider_count,
            self.curated_entry_count,
            self.curated_paper_count,
            self.curated_duplicate_count,
            self.overlap_count,
            self.doi_overlap_count,
            self.title_author_overlap_count,
            self.unique_curated_count,
            self.merged_count,
            self.curated_missing_date,
            self.curated_missing_abstract,
            self.curated_missing_pdf,
            self.curated_missing_venue,
            self.curated_missing_authors,
            self.overlap_abstract_enrichments,
            self.overlap_pdf_enrichments,
        );

        output.push_str("## Source ingestion\n\n");
        output.push_str(
            "| Source | Entries | Accepted | Excluded section | Irrelevant | Unavailable | No stable ID |\n",
        );
        output.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
        for diagnostic in &self.diagnostics {
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                diagnostic.source_name,
                diagnostic.entries_seen,
                diagnostic.accepted,
                diagnostic.excluded_section,
                diagnostic.irrelevant,
                diagnostic.unavailable,
                diagnostic.missing_stable_id,
            ));
        }

        output.push_str("\n## Curated section distribution\n\n");
        output.push_str("| Section | Recent papers |\n|---|---:|\n");
        for (category, count) in &self.categories {
            output.push_str(&format!(
                "| {} | {} |\n",
                escape_table_cell(category),
                count
            ));
        }

        output.push_str("\n## Unique curated papers for manual review\n\n");
        if self.discoveries.is_empty() {
            output.push_str("No unique curated papers were found.\n");
        } else {
            for discovery in &self.discoveries {
                output.push_str(&format!("- {discovery}\n"));
            }
            if self.unique_curated_count > self.discoveries.len() {
                output.push_str(&format!(
                    "- … and {} more\n",
                    self.unique_curated_count - self.discoveries.len()
                ));
            }
        }
        output
    }
}

#[derive(Clone, Copy)]
enum MatchKind {
    Doi,
    TitleAuthors,
}

fn match_kind(left: &Work, right: &Work) -> Option<MatchKind> {
    let left_keys = version_keys(left);
    let right_keys = version_keys(right);
    left_keys.iter().find_map(|left_key| {
        right_keys.contains(left_key).then_some(match left_key {
            VersionKey::Doi(_) => MatchKind::Doi,
            VersionKey::AuthorIds(_, _) | VersionKey::AuthorNames(_, _) => MatchKind::TitleAuthors,
        })
    })
}

fn escape_table_cell(value: &str) -> String {
    value.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openalex::{Author, Authorship, Location, Source};
    use crate::provenance::DiscoverySource;

    fn work(id: &str, doi: Option<&str>, title: &str, author: &str) -> Work {
        Work {
            id: Some(id.to_string()),
            doi: doi.map(str::to_string),
            title: Some(title.to_string()),
            display_name: None,
            publication_date: Some("2026-01-01".to_string()),
            latest_version_date: None,
            collection_date: None,
            cited_by_count: None,
            authorships: Some(vec![Authorship {
                author: Some(Author {
                    id: None,
                    display_name: Some(author.to_string()),
                }),
                raw_author_name: Some(author.to_string()),
            }]),
            primary_location: Some(Location {
                landing_page_url: None,
                pdf_url: None,
                source: Some(Source {
                    display_name: Some("bioRxiv".to_string()),
                }),
                version: None,
            }),
            best_oa_location: None,
            abstract_inverted_index: None,
            abstract_text_override: None,
            alternate_links: Vec::new(),
            matched_author_names: Vec::new(),
            discovery_sources: vec![DiscoverySource::curated_collection(
                "example".to_string(),
                "Example".to_string(),
                "https://example.com".to_string(),
            )],
            curated_categories: vec!["3.4 Diffusion".to_string()],
        }
    }

    #[test]
    fn reports_doi_title_author_overlap_and_unique_discoveries() {
        let mut provider_doi = work(
            "W1",
            Some("https://doi.org/10.1/shared"),
            "Published title",
            "Ada Lovelace",
        );
        provider_doi.abstract_text_override = Some("Abstract".to_string());
        let provider_title = work("W2", None, "Same title", "Grace Hopper");
        let provider = vec![provider_doi, provider_title];
        let curated = vec![
            work(
                "doi:10.1/shared",
                Some("https://doi.org/10.1/shared"),
                "Preprint title",
                "Ada Lovelace",
            ),
            work("arxiv:1", None, "Same title", "Grace Hopper"),
            work("arxiv:2", None, "New discovery", "Alan Turing"),
        ];

        let report = compare(&provider, &curated, Vec::new());

        assert_eq!(report.overlap_count, 2);
        assert_eq!(report.doi_overlap_count, 1);
        assert_eq!(report.title_author_overlap_count, 1);
        assert_eq!(report.unique_curated_count, 1);
        assert_eq!(report.merged_count, 3);
        assert_eq!(report.overlap_abstract_enrichments, 1);
        assert_eq!(report.discoveries, vec!["2026-01-01 — New discovery"]);
    }

    #[test]
    fn renders_ingestion_and_metadata_sections() {
        let diagnostics = vec![SourceDiagnostics {
            source_name: "source".to_string(),
            entries_seen: 10,
            accepted: 6,
            excluded_section: 2,
            irrelevant: 0,
            unavailable: 1,
            missing_stable_id: 1,
        }];
        let report = compare(
            &[],
            &[work("W1", None, "A | B", "Ada Lovelace")],
            diagnostics,
        );

        let markdown = report.render_markdown("bioml", "source", "2025-01-01", "OpenAlex");

        assert!(markdown.contains("| source | 10 | 6 | 2 | 0 | 1 | 1 |"));
        assert!(markdown.contains("| 3.4 Diffusion | 1 |"));
        assert!(markdown.contains("- 2026-01-01 — A | B"));
    }

    #[test]
    fn deduplicates_curated_versions_before_counting_discoveries() {
        let first = work(
            "doi:10.1/shared",
            Some("https://doi.org/10.1/shared"),
            "First title",
            "Ada Lovelace",
        );
        let second = work(
            "W2",
            Some("https://doi.org/10.1/shared"),
            "Published title",
            "Ada Lovelace",
        );

        let report = compare(&[], &[first, second], Vec::new());

        assert_eq!(report.curated_paper_count, 1);
        assert_eq!(report.curated_duplicate_count, 1);
        assert_eq!(report.unique_curated_count, 1);
    }
}
