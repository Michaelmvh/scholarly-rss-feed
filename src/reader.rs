#[derive(Clone, Debug)]
pub struct Feed {
    pub title: String,
    pub description: String,
    pub publications: Vec<Publication>,
}

#[derive(Clone, Debug)]
pub struct Publication {
    pub id: Option<String>,
    pub title: String,
    pub link: Option<String>,
    pub publication_date: Option<String>,
    pub venue: Option<String>,
    pub authors: Vec<Author>,
    pub abstract_text: Option<String>,
    pub curated_sources: Vec<Attribution>,
    pub curated_categories: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Author {
    pub name: String,
    pub matched_feed: bool,
}

#[derive(Clone, Debug)]
pub struct Attribution {
    pub name: String,
    pub url: String,
}

const STYLES: &str = r#"
:root { color-scheme: light; font-family: ui-serif, Georgia, Cambria, "Times New Roman", serif; background: #f7f5f0; color: #272522; }
* { box-sizing: border-box; }
body { margin: 0; }
main { width: min(48rem, calc(100% - 2rem)); margin: 0 auto; padding: clamp(2rem, 7vw, 5rem) 0; }
header { border-bottom: 1px solid #d8d2c7; padding-bottom: 2rem; }
h1 { max-width: 18ch; margin: 0 0 .75rem; font-size: clamp(2.25rem, 8vw, 4.5rem); font-weight: 500; line-height: .98; letter-spacing: -.035em; }
.intro { max-width: 42rem; margin: 0; color: #625e57; font-size: 1.05rem; line-height: 1.6; }
.feed-meta { display: flex; flex-wrap: wrap; justify-content: space-between; gap: .75rem; margin: 1.25rem 0 0; font: .85rem/1.4 ui-sans-serif, system-ui, sans-serif; color: #625e57; }
a { color: #175e54; text-decoration-thickness: .08em; text-underline-offset: .16em; }
a:hover { color: #0d3d36; }
article { padding: 2rem 0; border-bottom: 1px solid #d8d2c7; }
h2 { margin: .35rem 0 .75rem; font-size: clamp(1.35rem, 4vw, 1.8rem); font-weight: 500; line-height: 1.2; letter-spacing: -.015em; }
h2 a { color: inherit; text-decoration-color: #9db8b3; }
.metadata, .authors, .back { font-family: ui-sans-serif, system-ui, sans-serif; color: #625e57; }
.metadata { margin: 0; font-size: .78rem; font-weight: 650; letter-spacing: .045em; text-transform: uppercase; }
.authors { margin: 0; font-size: .9rem; line-height: 1.55; }
.notable-author { color: #175e54; font-weight: 750; }
.author-key, .author-note { margin: .75rem 0 0; font: .8rem/1.45 ui-sans-serif, system-ui, sans-serif; color: #625e57; }
.provenance { margin: .75rem 0 0; font: .78rem/1.5 ui-sans-serif, system-ui, sans-serif; color: #625e57; }
.empty { padding: 3rem 0; color: #625e57; }
.back { display: inline-block; margin-bottom: 2.5rem; font-size: .9rem; }
.article-detail { padding-top: 0; border: 0; }
.article-detail h1 { max-width: 22ch; font-size: clamp(2rem, 7vw, 3.75rem); line-height: 1.06; }
.abstract { margin-top: 2.25rem; color: #48453f; font-size: 1.08rem; line-height: 1.75; }
.abstract h2 { font: 650 .82rem/1.4 ui-sans-serif, system-ui, sans-serif; letter-spacing: .06em; text-transform: uppercase; }
.actions { display: flex; flex-wrap: wrap; gap: .75rem 1.25rem; align-items: center; margin-top: 2rem; font: 600 .9rem/1.4 ui-sans-serif, system-ui, sans-serif; }
.primary-action { display: inline-block; padding: .7rem 1rem; border-radius: .25rem; background: #175e54; color: #fff; text-decoration: none; }
.primary-action:hover { background: #0d3d36; color: #fff; }
@media (prefers-reduced-motion: no-preference) { html { scroll-behavior: smooth; } }
"#;

pub fn render_feed(feed: &Feed, params: &[(String, String)]) -> String {
    let title = escape_html(&feed.title);
    let description = escape_html(&feed.description);
    let rss_url = escape_html(&raw_feed_url(params));
    let item_count = feed.publications.len();
    let item_label = if item_count == 1 {
        "1 publication".to_string()
    } else {
        format!("{item_count} publications")
    };

    let mut articles = String::new();
    for publication in &feed.publications {
        let title_markup = match publication.id.as_deref() {
            Some(article_id) => format!(
                r#"<a href="{}">{}</a>"#,
                escape_html(&article_url(params, article_id)),
                escape_html(&publication.title)
            ),
            None => escape_html(&publication.title),
        };
        let (date, separator, venue) = render_metadata(publication);
        let authors = render_authors(&publication.authors);
        let provenance = render_provenance(publication);

        articles.push_str(&format!(
            r#"<article>
  <p class="metadata">{date}{separator}{venue}</p>
  <h2>{title_markup}</h2>
  {authors}
  {provenance}
</article>
"#
        ));
    }
    if articles.is_empty() {
        articles.push_str(r#"<p class="empty">No recent publications found.</p>"#);
    }
    let author_key = if feed
        .publications
        .iter()
        .any(|publication| publication.authors.iter().any(|author| author.matched_feed))
    {
        r#"<p class="author-key"><strong class="notable-author">Highlighted authors</strong> matched this feed.</p>"#
    } else {
        ""
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="description" content="{description}">
  <link rel="alternate" type="application/rss+xml" title="{title}" href="{rss_url}">
  <title>{title}</title>
  <style>{STYLES}</style>
</head>
<body>
  <main>
    <header>
      <h1>{title}</h1>
      <p class="intro">{description}</p>
      <p class="feed-meta"><span>{item_label}</span><a href="{rss_url}">View raw RSS</a></p>
      {author_key}
    </header>
    {articles}
  </main>
</body>
</html>
"#
    )
}

pub fn render_article(
    feed: &Feed,
    article_id: &str,
    params: &[(String, String)],
) -> Option<String> {
    let publication = feed
        .publications
        .iter()
        .find(|publication| publication.id.as_deref() == Some(article_id))?;
    let feed_title = escape_html(&feed.title);
    let item_title = escape_html(&publication.title);
    let rss_url = escape_html(&raw_feed_url(params));
    let back_url = escape_html(&reader_url(params));
    let (date, separator, venue) = render_metadata(publication);
    let authors = render_authors(&publication.authors);
    let provenance = render_provenance(publication);
    let author_note = if publication.authors.iter().any(|author| author.matched_feed) {
        r#"<p class="author-note"><strong class="notable-author">Highlighted authors</strong> matched this feed.</p>"#
    } else {
        ""
    };
    let abstract_text = publication
        .abstract_text
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(escape_html)
        .unwrap_or_else(|| String::from("No abstract is available for this publication."));
    let article_link = publication
        .link
        .as_deref()
        .map(|link| {
            format!(
                r#"<a class="primary-action" href="{}" rel="external">Read full article <span aria-hidden="true">↗</span></a>"#,
                escape_html(link)
            )
        })
        .unwrap_or_default();

    Some(format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="description" content="Abstract for {item_title}">
  <link rel="alternate" type="application/rss+xml" title="{feed_title}" href="{rss_url}">
  <title>{item_title} · {feed_title}</title>
  <style>{STYLES}</style>
</head>
<body>
  <main>
    <a class="back" href="{back_url}">← Back to all publications</a>
    <article class="article-detail">
      <p class="metadata">{date}{separator}{venue}</p>
      <h1>{item_title}</h1>
      {authors}
      {provenance}
      {author_note}
      <section class="abstract" aria-labelledby="abstract-heading">
        <h2 id="abstract-heading">Abstract</h2>
        <p>{abstract_text}</p>
      </section>
      <p class="actions">{article_link}<a href="{rss_url}">View raw RSS</a></p>
    </article>
  </main>
</body>
</html>
"#
    ))
}

fn render_metadata(publication: &Publication) -> (String, &'static str, String) {
    let date = publication
        .publication_date
        .as_deref()
        .map(format_reader_date)
        .map(|value| format!(r#"<time>{}</time>"#, escape_html(&value)))
        .unwrap_or_default();
    let venue = publication
        .venue
        .as_deref()
        .map(escape_html)
        .unwrap_or_default();
    let separator = if !date.is_empty() && !venue.is_empty() {
        r#"<span aria-hidden="true"> · </span>"#
    } else {
        ""
    };

    (date, separator, venue)
}

fn render_authors(authors: &[Author]) -> String {
    if authors.is_empty() {
        return String::new();
    }

    let authors = authors
        .iter()
        .map(|author| {
            let name = escape_html(&author.name);
            if author.matched_feed {
                format!(r#"<strong class="notable-author">{name}</strong>"#)
            } else {
                name
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    format!(r#"<p class="authors">{authors}</p>"#)
}

fn render_provenance(publication: &Publication) -> String {
    if publication.curated_sources.is_empty() {
        return String::new();
    }

    let sources = publication
        .curated_sources
        .iter()
        .map(|source| {
            format!(
                r#"<a href="{}" rel="external">{}</a>"#,
                escape_html(&source.url),
                escape_html(&source.name)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let categories = if publication.curated_categories.is_empty() {
        String::new()
    } else {
        format!(
            " · {}",
            escape_html(&publication.curated_categories.join(" · "))
        )
    };

    format!(r#"<p class="provenance">Curated by {sources}{categories}</p>"#)
}

fn raw_feed_url(params: &[(String, String)]) -> String {
    let query = feed_query(params);
    if query.is_empty() {
        String::from("?rss")
    } else {
        format!("?{query}&rss")
    }
}

fn reader_url(params: &[(String, String)]) -> String {
    let query = feed_query(params);
    if query.is_empty() {
        String::from("/")
    } else {
        format!("?{query}")
    }
}

fn article_url(params: &[(String, String)], article_id: &str) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    let query = feed_query(params);
    if !query.is_empty() {
        serializer.extend_pairs(url::form_urlencoded::parse(query.as_bytes()));
    }
    serializer.append_pair("article", article_id);
    format!("?{}", serializer.finish())
}

fn feed_query(params: &[(String, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in params
        .iter()
        .filter(|(name, _)| name != "rss" && name != "article")
    {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}

fn format_reader_date(date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|parsed| parsed.format("%B %-d, %Y").to_string())
        .unwrap_or_else(|_| date.to_string())
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_feed() -> Feed {
        Feed {
            title: "Research & <Development>".to_string(),
            description: "Recent \"research\"".to_string(),
            publications: vec![Publication {
                id: Some("https://openalex.org/W1".to_string()),
                title: "<script>alert('title')</script>".to_string(),
                link: Some("https://example.com/?a=1&b=2".to_string()),
                publication_date: Some("2026-08-27".to_string()),
                venue: Some("Example & Journal".to_string()),
                authors: vec![
                    Author {
                        name: "Ada Lovelace".to_string(),
                        matched_feed: true,
                    },
                    Author {
                        name: "Grace Hopper".to_string(),
                        matched_feed: true,
                    },
                    Author {
                        name: "Alan Turing".to_string(),
                        matched_feed: false,
                    },
                ],
                abstract_text: Some("<strong>abstract</strong>".to_string()),
                curated_sources: vec![Attribution {
                    name: "Curated Source".to_string(),
                    url: "https://example.com/source?a=1&b=2".to_string(),
                }],
                curated_categories: vec!["Protein design".to_string()],
            }],
        }
    }

    #[test]
    fn raw_feed_url_preserves_existing_parameters() {
        let params = vec![
            ("feed".to_string(), "my field".to_string()),
            ("author_id".to_string(), "A1".to_string()),
        ];

        assert_eq!(raw_feed_url(&params), "?feed=my+field&author_id=A1&rss");
        assert_eq!(raw_feed_url(&[]), "?rss");
    }

    #[test]
    fn reader_escapes_content_and_links_to_article_preview() {
        let params = vec![("feed".to_string(), "myfield".to_string())];
        let html = render_feed(&sample_feed(), &params);

        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("Research &amp; &lt;Development&gt;"));
        assert!(html.contains("&lt;script&gt;alert(&#39;title&#39;)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("article=https%3A%2F%2Fopenalex.org%2FW1"));
        assert!(html.contains("href=\"?feed=myfield&amp;rss\""));
        assert!(html.contains("Curated by"));
        assert!(html.contains("https://example.com/source?a=1&amp;b=2"));
    }

    #[test]
    fn article_preview_links_externally_and_highlights_all_matching_authors() {
        let params = vec![("feed".to_string(), "myfield".to_string())];
        let html = render_article(&sample_feed(), "https://openalex.org/W1", &params).unwrap();

        assert!(html.contains("https://example.com/?a=1&amp;b=2"));
        assert!(html.contains("&lt;strong&gt;abstract&lt;/strong&gt;"));
        assert!(html.contains(
            r#"<strong class="notable-author">Ada Lovelace</strong>, <strong class="notable-author">Grace Hopper</strong>, Alan Turing"#
        ));
    }
}
