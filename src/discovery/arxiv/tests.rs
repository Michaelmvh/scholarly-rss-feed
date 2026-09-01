use super::*;

#[test]
fn validates_only_pilot_categories() {
    assert!(validate_categories(&["q-bio.BM".to_string(), "q-bio.QM".to_string()]).is_ok());
    assert_eq!(
        validate_categories(&["cs.LG".to_string()]).unwrap_err(),
        "Unknown arXiv category \"cs.LG\"."
    );
}

#[test]
fn builds_bounded_atom_query() {
    assert_eq!(
        atom_query(
            &["q-bio.BM".to_string(), "q-bio.QM".to_string()],
            "2026-08-01",
            "2026-09-01"
        )
        .unwrap(),
        "(cat:q-bio.BM OR cat:q-bio.QM) AND submittedDate:[202608010000 TO 202609012359]"
    );
}

#[test]
fn parses_modern_and_legacy_arxiv_ids() {
    assert_eq!(
        parse_arxiv_id("https://arxiv.org/abs/2608.30337v2").as_deref(),
        Some("2608.30337")
    );
    assert_eq!(
        parse_arxiv_id("oai:arXiv.org:q-bio/0601001v1").as_deref(),
        Some("q-bio/0601001")
    );
}

#[test]
fn derives_month_date_from_arxiv_identifiers() {
    assert_eq!(
        arxiv_month_date("2608.30337").as_deref(),
        Some("2026-08-01")
    );
    assert_eq!(
        arxiv_month_date("q-bio/0601001").as_deref(),
        Some("2006-01-01")
    );
}

#[test]
fn builds_arxiv_work_with_matching_category_provenance() {
    let categories = vec!["cs.LG".to_string(), "q-bio.BM".to_string()];
    let configured = vec!["q-bio.BM".to_string(), "q-bio.QM".to_string()];

    let work = build_work(
        "2608.30337",
        "  A\n title ",
        Some(" An abstract. "),
        ["Ada Lovelace"],
        "2026-08-31",
        "2026-09-01",
        &categories,
        &configured,
        "https://arxiv.org/abs/2608.30337".to_string(),
        "https://arxiv.org/pdf/2608.30337".to_string(),
        Some("http://creativecommons.org/licenses/by/4.0/"),
        Some("10.1000/published"),
    );

    assert_eq!(work.title.as_deref(), Some("A title"));
    assert_eq!(
        work.doi.as_deref(),
        Some("https://doi.org/10.48550/arXiv.2608.30337")
    );
    assert_eq!(work.author_names(), vec!["Ada Lovelace"]);
    assert_eq!(work.abstract_text().as_deref(), Some("An abstract."));
    assert_eq!(work.published_doi.as_deref(), Some("10.1000/published"));
    assert_eq!(
        work.discovery_sources,
        vec![DiscoverySource::arxiv_category("q-bio.BM", "Biomolecules")]
    );
}
