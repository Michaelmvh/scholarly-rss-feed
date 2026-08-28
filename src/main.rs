mod config;
mod curated;
mod evaluation;
mod google_scholar;
mod openalex;
mod reader;

use crate::config::{Config, FeedConfig};
use crate::openalex::{
    normalize_id, Author, AuthorsResponse, SourceRecord, SourcesResponse, Work, WorksResponse,
    API_BASE,
};
use chrono::{Duration, NaiveDate, NaiveTime, Utc};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::http::Error;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lazy_static::lazy_static;
use parking_lot::RwLock;
use rss::extension::dublincore::DublinCoreExtension;
use rss::{Category, Channel, ChannelBuilder, GuidBuilder, ItemBuilder, Source, TextInput};
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration as StdDuration;
use tokio::net::TcpListener;
use tokio::time::Instant;

lazy_static! {
    static ref RSS_CHANNELS: Arc<RwLock<HashMap<FeedRequest, GeneratedFeed>>> =
        Arc::new(RwLock::new(HashMap::new()));
    static ref FEED_BUILDS: FeedBuilds = Arc::new(RwLock::new(HashMap::new()));
    pub static ref CLIENT: reqwest::Client = reqwest::Client::builder()
        .user_agent("scholarly-rss-feed")
        .connect_timeout(StdDuration::from_secs(10))
        .timeout(StdDuration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");
}

static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

const DEFAULT_FROM_DAYS: u32 = 365;

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
enum Provider {
    OpenAlex,
    GoogleScholar,
}

#[derive(Debug)]
enum ResolveFeedError {
    InvalidRequest(String),
    Provider(String),
}

impl Provider {
    fn configured() -> Result<Self, String> {
        Self::parse(&env::var("GSRF_PROVIDER").unwrap_or_else(|_| "openalex".to_string()))
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openalex" => Ok(Self::OpenAlex),
            "google-scholar" | "google_scholar" | "scholar" => Ok(Self::GoogleScholar),
            other => Err(format!(
                "Unknown GSRF_PROVIDER \"{other}\"; expected \"openalex\" or \"google-scholar\"."
            )),
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::OpenAlex => "OpenAlex",
            Self::GoogleScholar => "Google Scholar",
        }
    }

    fn site_url(self) -> &'static str {
        match self {
            Self::OpenAlex => "https://openalex.org",
            Self::GoogleScholar => "https://scholar.google.com",
        }
    }

    fn search_url(self) -> &'static str {
        match self {
            Self::OpenAlex => "https://openalex.org/works",
            Self::GoogleScholar => "https://scholar.google.com/scholar",
        }
    }
}

#[derive(Clone)]
struct GeneratedFeed {
    channel: Channel,
    reader: reader::Feed,
}

struct CliArgs {
    address: String,
    config_path: PathBuf,
    comparison: Option<ComparisonArgs>,
}

struct ComparisonArgs {
    feed_name: String,
    source_name: String,
}

type FeedCacheKey = (
    Provider,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    String,
    Vec<String>,
);
type FeedBuild = Arc<tokio::sync::OnceCell<Result<GeneratedFeed, String>>>;
type FeedBuilds = Arc<RwLock<HashMap<FeedCacheKey, FeedBuild>>>;

/// Fully-resolved description of a feed, used as the cache key.
#[derive(Debug, Hash, Eq, PartialEq, Clone)]
struct FeedRequest {
    provider: Provider,
    /// Normalized, deduped, sorted OpenAlex author ids.
    author_ids: Vec<String>,
    /// Author names queried by the archived Google Scholar provider.
    google_scholar_authors: Vec<String>,
    /// Curated paper collections included in the feed.
    curated_sources: Vec<String>,
    /// Normalized, deduped, sorted OpenAlex source (journal) ids.
    source_ids: Vec<String>,
    /// Earliest publication date (YYYY-MM-DD).
    from: String,
    /// Sorted OpenAlex topic ids.
    topics: Vec<String>,
    /// Optional channel title carried alongside (not part of identity below).
    title: Option<String>,
}

impl FeedRequest {
    fn cache_key(&self) -> FeedCacheKey {
        (
            self.provider,
            self.author_ids.clone(),
            self.google_scholar_authors.clone(),
            self.curated_sources.clone(),
            self.source_ids.clone(),
            self.from.clone(),
            self.topics.clone(),
        )
    }
}

#[tokio::main]
async fn main() {
    let cli = match parse_cli_args_from(env::args().skip(1)) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    CONFIG_PATH.set(cli.config_path.clone()).ok();

    if let Some(comparison) = cli.comparison {
        if let Err(error) = run_curated_comparison(&comparison, &cli.config_path).await {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }

    let addr = SocketAddr::from_str(&cli.address).unwrap();

    println!("Listening on {}...", cli.address);
    println!("Using config file: {}", cli.config_path.display());
    let listener = TcpListener::bind(addr).await.unwrap();

    println!("Server started");
    let mut last_update = Instant::now();
    loop {
        // Clear hourly so feeds refresh and the cache doesn't grow unbounded.
        if last_update.elapsed() >= StdDuration::from_secs(3600) {
            println!("Clearing cache");
            RSS_CHANNELS.write().clear();
            FEED_BUILDS
                .write()
                .retain(|_, build| Arc::strong_count(build) > 1);
            last_update = Instant::now();
        }

        if let Ok((stream, _)) = listener.accept().await {
            let io = TokioIo::new(stream);

            tokio::task::spawn(async move {
                match http1::Builder::new()
                    .serve_connection(io, service_fn(serve_feed))
                    .await
                {
                    Ok(_) => (),
                    Err(err) => eprintln!("Error serving connection: {:?}", err),
                }
            });
        }
    }
}

/// Parse CLI args: first non-flag positional is the bind address, `--config <path>`
/// selects the feeds config file, and `--compare-curated <feed> <source>` runs
/// an evaluation instead of the server.
fn parse_cli_args_from(args: impl IntoIterator<Item = String>) -> Result<CliArgs, String> {
    let mut address: Option<String> = None;
    let mut config: Option<String> = None;
    let mut comparison = None;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                config = Some(
                    args.next()
                        .ok_or_else(|| "--config requires a path".to_string())?,
                );
            }
            "--compare-curated" => {
                let feed_name = args.next().ok_or_else(|| {
                    "--compare-curated requires a feed name and curated source".to_string()
                })?;
                let source_name = args.next().ok_or_else(|| {
                    "--compare-curated requires a feed name and curated source".to_string()
                })?;
                comparison = Some(ComparisonArgs {
                    feed_name,
                    source_name,
                });
            }
            flag if flag.starts_with("--") => {
                return Err(format!("Unknown option \"{flag}\"."));
            }
            other => {
                if address.is_none() {
                    address = Some(other.to_string());
                } else {
                    return Err(format!("Unexpected positional argument \"{other}\"."));
                }
            }
        }
    }

    let address = address.unwrap_or_else(|| "127.0.0.1:3005".to_string());
    let config = config
        .or_else(|| env::var("GSRF_CONFIG").ok())
        .unwrap_or_else(|| "feeds.toml".to_string());

    Ok(CliArgs {
        address,
        config_path: PathBuf::from(config),
        comparison,
    })
}

async fn run_curated_comparison(
    comparison: &ComparisonArgs,
    config_path: &std::path::Path,
) -> Result<(), String> {
    let config = Config::load(config_path);
    let provider = Provider::configured()?;
    let params = vec![("feed".to_string(), comparison.feed_name.clone())];
    let request = resolve_feed_request(&params, &config, provider)
        .await
        .map_err(|error| match error {
            ResolveFeedError::InvalidRequest(message) | ResolveFeedError::Provider(message) => {
                message
            }
        })?
        .ok_or_else(|| {
            format!(
                "Feed \"{}\" has no provider identifiers to compare.",
                comparison.feed_name
            )
        })?;
    curated::validate_sources(std::slice::from_ref(&comparison.source_name))?;

    let (provider_works, curated_evaluation) = tokio::join!(
        fetch_provider_works(&request),
        curated::fetch_sources_for_evaluation(
            &CLIENT,
            std::slice::from_ref(&comparison.source_name),
            &request.from,
        ),
    );
    let provider_works = provider_works?;
    let curated_evaluation = curated_evaluation?;
    let report = evaluation::compare(
        &provider_works,
        &curated_evaluation.works,
        curated_evaluation.diagnostics,
    );
    print!(
        "{}",
        report.render_markdown(
            &comparison.feed_name,
            &comparison.source_name,
            &request.from,
            provider.display_name(),
        )
    );
    Ok(())
}

async fn serve_feed(request: Request<Incoming>) -> Result<Response<Full<Bytes>>, Error> {
    // Preserve repeated query params (e.g. multiple ?author_id=).
    let params: Vec<(String, String)> = request
        .uri()
        .query()
        .map(|v| {
            url::form_urlencoded::parse(v.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();

    let config = Config::load(config_path());
    let provider = match Provider::configured() {
        Ok(provider) => provider,
        Err(message) => {
            return Response::builder()
                .header("Content-Type", "text/plain; charset=utf-8")
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from(message)));
        }
    };

    let feed_request = match resolve_feed_request(&params, &config, provider).await {
        Ok(Some(fr)) => fr,
        Ok(None) => {
            return Response::builder()
                .header("Access-Control-Allow-Origin", "*")
                .status(StatusCode::OK)
                .body(Full::new(Bytes::from(
                    "No authors or journals specified. Provide ?author_id=, ?orcid=, \
                     ?author=, ?source_id=, ?issn=, or ?journal=, or configure feeds in \
                     the config file and use ?feed=<name>.",
                )));
        }
        Err(ResolveFeedError::InvalidRequest(message)) => {
            return Response::builder()
                .header("Access-Control-Allow-Origin", "*")
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::from(message)));
        }
        Err(ResolveFeedError::Provider(message)) => {
            eprintln!("{message}");
            return Response::builder()
                .header("Content-Type", "text/plain; charset=utf-8")
                .status(StatusCode::BAD_GATEWAY)
                .body(Full::new(Bytes::from(message)));
        }
    };

    let feed = match generate_feed_if_needed(feed_request).await {
        Ok(feed) => feed,
        Err(message) => {
            eprintln!("{message}");
            return Response::builder()
                .header("Content-Type", "text/plain; charset=utf-8")
                .status(StatusCode::BAD_GATEWAY)
                .body(Full::new(Bytes::from(message)));
        }
    };
    let (status, content_type, body) = if has_param(&params, "rss") {
        (
            StatusCode::OK,
            "text/xml; charset=utf-8",
            Bytes::from(feed.channel.to_string()),
        )
    } else if let Some(article_id) = first_param(&params, "article") {
        match reader::render_article(&feed.reader, &article_id, &params) {
            Some(html) => (
                StatusCode::OK,
                "text/html; charset=utf-8",
                Bytes::from(html),
            ),
            None => (
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                Bytes::from("Publication not found in this feed."),
            ),
        }
    } else {
        (
            StatusCode::OK,
            "text/html; charset=utf-8",
            Bytes::from(reader::render_feed(&feed.reader, &params)),
        )
    };

    Response::builder()
        .header("Content-Type", content_type)
        .header("Access-Control-Allow-Origin", "*")
        .header(
            "Cache-Control",
            "public, max-age=300, s-maxage=7200, stale-while-revalidate=86400",
        )
        .status(status)
        .body(Full::new(body))
}

fn config_path() -> &'static PathBuf {
    CONFIG_PATH.get().expect("config path not initialized")
}

/// Collect the values of a repeated query parameter.
fn collect_param(params: &[(String, String)], key: &str) -> Vec<String> {
    params
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .filter(|v| !v.trim().is_empty())
        .collect()
}

fn first_param(params: &[(String, String)], key: &str) -> Option<String> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .filter(|v| !v.trim().is_empty())
}

fn has_param(params: &[(String, String)], key: &str) -> bool {
    params.iter().any(|(name, _)| name == key)
}

/// Merge a named feed (if any) with provider-compatible ad-hoc URL params and
/// build the cache key. Returns:
/// - `Ok(Some(req))` when the provider has enough identifiers,
/// - `Ok(None)` when no identifiers were specified at all,
/// - `Err(msg)` when a named feed was requested but not found.
async fn resolve_feed_request(
    params: &[(String, String)],
    config: &Config,
    provider: Provider,
) -> Result<Option<FeedRequest>, ResolveFeedError> {
    let feed_name = first_param(params, "feed");

    // Ad-hoc params.
    let adhoc_author_ids = collect_param(params, "author_id");
    let adhoc_orcids = collect_param(params, "orcid");
    let adhoc_authors = collect_param(params, "author");
    let adhoc_source_ids = collect_param(params, "source_id");
    let adhoc_issns = collect_param(params, "issn");
    let adhoc_journals = collect_param(params, "journal");
    let mut adhoc_topics = collect_param(params, "topic");
    adhoc_topics.extend(collect_param(params, "concept"));
    let adhoc_from = first_param(params, "from");

    let has_adhoc = !adhoc_author_ids.is_empty()
        || !adhoc_orcids.is_empty()
        || !adhoc_authors.is_empty()
        || !adhoc_source_ids.is_empty()
        || !adhoc_issns.is_empty()
        || !adhoc_journals.is_empty();

    // Determine the feed config to use.
    let feed: Option<&FeedConfig> = match &feed_name {
        Some(name) => match config.feeds.get(name) {
            Some(f) => Some(f),
            None => {
                return Err(ResolveFeedError::InvalidRequest(format!(
                    "Unknown feed \"{name}\"."
                )))
            }
        },
        None => {
            if has_adhoc {
                None
            } else {
                // Bare request: serve the default feed if configured.
                match &config.default_feed {
                    Some(name) => match config.feeds.get(name) {
                        Some(f) => Some(f),
                        None => {
                            return Err(ResolveFeedError::InvalidRequest(format!(
                                "Configured default_feed \"{name}\" not found."
                            )))
                        }
                    },
                    None => return Ok(None),
                }
            }
        }
    };

    let empty = FeedConfig::default();
    let feed = feed.unwrap_or(&empty);
    let from = adhoc_from
        .or_else(|| feed.from.clone())
        .unwrap_or_else(|| default_from_date(config.settings.from_days));
    let mut curated_sources = feed.curated_sources.clone();
    curated_sources.sort();
    curated_sources.dedup();
    curated::validate_sources(&curated_sources).map_err(ResolveFeedError::InvalidRequest)?;

    if provider == Provider::GoogleScholar {
        let mut google_scholar_authors = feed
            .people
            .iter()
            .map(|person| {
                person
                    .google_scholar_name
                    .as_deref()
                    .unwrap_or(&person.name)
            })
            .chain(feed.google_scholar_authors.iter().map(String::as_str))
            .chain(feed.authors.iter().map(String::as_str))
            .chain(adhoc_authors.iter().map(String::as_str))
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>();
        google_scholar_authors.sort();
        google_scholar_authors.dedup();

        if google_scholar_authors.is_empty() && curated_sources.is_empty() {
            return Err(ResolveFeedError::InvalidRequest(
                "The Google Scholar provider requires at least one \
                 people entry, legacy google_scholar_authors entry, ?author= parameter, \
                 or curated source."
                    .to_string(),
            ));
        }

        return Ok(Some(FeedRequest {
            provider,
            author_ids: Vec::new(),
            google_scholar_authors,
            curated_sources,
            source_ids: Vec::new(),
            from,
            topics: Vec::new(),
            title: feed.title.clone(),
        }));
    }

    // Gather author identifiers from feed config + ad-hoc params.
    let mut author_ids: Vec<String> = Vec::new();
    for id in feed
        .people
        .iter()
        .filter_map(|person| person.openalex_id.as_deref())
    {
        author_ids.push(normalize_id(id));
    }
    for id in feed.author_ids.iter().chain(adhoc_author_ids.iter()) {
        author_ids.push(normalize_id(id));
    }

    for orcid in feed.orcids.iter().chain(adhoc_orcids.iter()) {
        match resolve_orcid(orcid)
            .await
            .map_err(ResolveFeedError::Provider)?
        {
            Some(id) => author_ids.push(id),
            None => eprintln!("Could not resolve ORCID \"{orcid}\""),
        }
    }

    for name in feed.authors.iter().chain(adhoc_authors.iter()) {
        match resolve_author_name(name)
            .await
            .map_err(ResolveFeedError::Provider)?
        {
            Some((id, display)) => {
                println!("Resolved author \"{name}\" -> {id} ({display})");
                author_ids.push(id);
            }
            None => eprintln!("Could not resolve author name \"{name}\""),
        }
    }

    // Normalize the id set: dedupe + sort for a stable cache key.
    author_ids.sort();
    author_ids.dedup();

    // Gather journal (source) identifiers from feed config + ad-hoc params.
    let mut source_ids: Vec<String> = Vec::new();
    for id in feed.source_ids.iter().chain(adhoc_source_ids.iter()) {
        source_ids.push(normalize_id(id));
    }

    for issn in feed.issns.iter().chain(adhoc_issns.iter()) {
        match resolve_issn(issn)
            .await
            .map_err(ResolveFeedError::Provider)?
        {
            Some(id) => source_ids.push(id),
            None => eprintln!("Could not resolve ISSN \"{issn}\""),
        }
    }

    for name in feed.journals.iter().chain(adhoc_journals.iter()) {
        match resolve_journal_name(name)
            .await
            .map_err(ResolveFeedError::Provider)?
        {
            Some((id, display)) => {
                println!("Resolved journal \"{name}\" -> {id} ({display})");
                source_ids.push(id);
            }
            None => eprintln!("Could not resolve journal name \"{name}\""),
        }
    }

    source_ids.sort();
    source_ids.dedup();

    // A feed needs at least one author or journal to produce anything.
    if author_ids.is_empty() && source_ids.is_empty() && curated_sources.is_empty() {
        return Ok(None);
    }

    // Topics.
    let mut topics: Vec<String> = feed
        .topics
        .iter()
        .chain(adhoc_topics.iter())
        .map(|t| normalize_id(t))
        .collect();
    topics.sort();
    topics.dedup();

    Ok(Some(FeedRequest {
        provider,
        author_ids,
        google_scholar_authors: Vec::new(),
        curated_sources,
        source_ids,
        from,
        topics,
        title: feed.title.clone(),
    }))
}

/// Compute a YYYY-MM-DD date `from_days` (or the default window) in the past.
fn default_from_date(from_days: Option<u32>) -> String {
    let days = from_days.unwrap_or(DEFAULT_FROM_DAYS) as i64;
    let date = Utc::now().date_naive() - Duration::days(days);
    date.format("%Y-%m-%d").to_string()
}

fn mailto() -> Option<String> {
    env::var("GSRF_MAILTO")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Resolve an ORCID to a bare OpenAlex author id.
async fn resolve_orcid(orcid: &str) -> Result<Option<String>, String> {
    let orcid = orcid.trim();
    let orcid = orcid.rsplit('/').next().unwrap_or(orcid);
    let mut url = url::Url::parse(&format!("{API_BASE}/authors/https://orcid.org/{orcid}"))
        .map_err(|error| format!("Failed to build OpenAlex ORCID URL: {error}"))?;
    if let Some(m) = mailto() {
        url.query_pairs_mut().append_pair("mailto", &m);
    }
    let response = CLIENT
        .get(url)
        .send()
        .await
        .map_err(|error| format!("OpenAlex ORCID lookup failed: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let author = response
        .error_for_status()
        .map_err(|error| format!("OpenAlex ORCID lookup failed: {error}"))?
        .json::<Author>()
        .await
        .map_err(|error| format!("Failed to parse OpenAlex ORCID response: {error}"))?;
    let id = author
        .id
        .ok_or_else(|| "OpenAlex ORCID response did not contain an author ID".to_string())?;
    Ok(Some(normalize_id(&id)))
}

/// Resolve an author display name (best match) to (id, display_name).
async fn resolve_author_name(name: &str) -> Result<Option<(String, String)>, String> {
    let mut pairs = vec![
        ("search".to_string(), name.to_string()),
        ("per_page".to_string(), "1".to_string()),
    ];
    if let Some(m) = mailto() {
        pairs.push(("mailto".to_string(), m));
    }
    let url = url::Url::parse_with_params(&format!("{API_BASE}/authors"), &pairs)
        .map_err(|error| format!("Failed to build OpenAlex author search URL: {error}"))?;
    let response = CLIENT
        .get(url)
        .send()
        .await
        .map_err(|error| format!("OpenAlex author search failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("OpenAlex author search failed: {error}"))?
        .json::<AuthorsResponse>()
        .await
        .map_err(|error| format!("Failed to parse OpenAlex author search response: {error}"))?;
    let Some(author) = response.results.into_iter().next() else {
        return Ok(None);
    };
    let id = normalize_id(
        &author
            .id
            .ok_or_else(|| "OpenAlex author search result did not contain an ID".to_string())?,
    );
    let display = author.display_name.unwrap_or_else(|| id.clone());
    Ok(Some((id, display)))
}

/// Resolve an ISSN to a bare OpenAlex source id.
async fn resolve_issn(issn: &str) -> Result<Option<String>, String> {
    let issn = issn.trim();
    let mut url = url::Url::parse(&format!("{API_BASE}/sources/issn:{issn}"))
        .map_err(|error| format!("Failed to build OpenAlex ISSN URL: {error}"))?;
    if let Some(m) = mailto() {
        url.query_pairs_mut().append_pair("mailto", &m);
    }
    let response = CLIENT
        .get(url)
        .send()
        .await
        .map_err(|error| format!("OpenAlex ISSN lookup failed: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let source = response
        .error_for_status()
        .map_err(|error| format!("OpenAlex ISSN lookup failed: {error}"))?
        .json::<SourceRecord>()
        .await
        .map_err(|error| format!("Failed to parse OpenAlex ISSN response: {error}"))?;
    let id = source
        .id
        .ok_or_else(|| "OpenAlex ISSN response did not contain a source ID".to_string())?;
    Ok(Some(normalize_id(&id)))
}

/// Resolve a journal display name (best match) to (id, display_name).
async fn resolve_journal_name(name: &str) -> Result<Option<(String, String)>, String> {
    let mut pairs = vec![
        ("search".to_string(), name.to_string()),
        ("per_page".to_string(), "1".to_string()),
    ];
    if let Some(m) = mailto() {
        pairs.push(("mailto".to_string(), m));
    }
    let url = url::Url::parse_with_params(&format!("{API_BASE}/sources"), &pairs)
        .map_err(|error| format!("Failed to build OpenAlex journal search URL: {error}"))?;
    let response = CLIENT
        .get(url)
        .send()
        .await
        .map_err(|error| format!("OpenAlex journal search failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("OpenAlex journal search failed: {error}"))?
        .json::<SourcesResponse>()
        .await
        .map_err(|error| format!("Failed to parse OpenAlex journal search response: {error}"))?;
    let Some(source) = response.results.into_iter().next() else {
        return Ok(None);
    };
    let id = normalize_id(
        &source
            .id
            .ok_or_else(|| "OpenAlex journal search result did not contain an ID".to_string())?,
    );
    let display = source.display_name.unwrap_or_else(|| id.clone());
    Ok(Some((id, display)))
}

async fn generate_feed_if_needed(request: FeedRequest) -> Result<GeneratedFeed, String> {
    let key = request.cache_key();
    if let Some(feed) = find_cached(&key) {
        return Ok(feed);
    }

    let build = FEED_BUILDS
        .write()
        .entry(key.clone())
        .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
        .clone();
    let result = build
        .get_or_init(|| async { build_feed(&request).await })
        .await
        .clone();
    if let Ok(feed) = &result {
        RSS_CHANNELS.write().insert(request, feed.clone());
    }
    let mut builds = FEED_BUILDS.write();
    if builds
        .get(&key)
        .is_some_and(|current| Arc::ptr_eq(current, &build))
    {
        builds.remove(&key);
    }

    result
}

fn find_cached(key: &FeedCacheKey) -> Option<GeneratedFeed> {
    let channels = RSS_CHANNELS.read();
    channels
        .iter()
        .find(|(request, _)| &request.cache_key() == key)
        .map(|(_, feed)| feed.clone())
}

async fn build_feed(request: &FeedRequest) -> Result<GeneratedFeed, String> {
    println!(
        "Building RSS channel for authors [{}] journals [{}] curated sources [{}] from {}",
        request.author_ids.join(", "),
        request.source_ids.join(", "),
        request.curated_sources.join(", "),
        request.from
    );

    let (title, description) = channel_metadata(request);
    let provider_name = request.provider.display_name();

    let mut channel = ChannelBuilder::default()
        .title(title.clone())
        .link(request.provider.site_url())
        .description(description.clone())
        .language(String::from("en-US"))
        .generator(String::from("scholarly-rss-feed"))
        .ttl(String::from("60"))
        .docs(String::from("https://cyber.harvard.edu/rss/rss.html"))
        .text_input(TextInput {
            title: provider_name.to_string(),
            description: format!("Search {provider_name}"),
            name: String::from("q"),
            link: request.provider.search_url().to_string(),
        })
        .categories(vec![Category::from("Scientific Research")])
        .build();

    let works = fetch_works(request).await?;
    let items = works.iter().map(work_to_item).collect::<Vec<_>>();
    channel.set_items(items);

    let now = Utc::now().to_rfc2822();
    channel.set_pub_date(now.clone());
    channel.set_last_build_date(now);

    let publications = works.iter().map(work_to_publication).collect();

    Ok(GeneratedFeed {
        channel,
        reader: reader::Feed {
            title,
            description,
            publications,
        },
    })
}

fn channel_metadata(request: &FeedRequest) -> (String, String) {
    let provider_name = request.provider.display_name();
    if let Some(title) = &request.title {
        return (
            title.clone(),
            format!("{title}. Recent publications parsed from {provider_name}."),
        );
    }
    let authors = match request.provider {
        Provider::OpenAlex => request.author_ids.len(),
        Provider::GoogleScholar => request.google_scholar_authors.len(),
    };
    let journals = request.source_ids.len();
    let curated_sources = request.curated_sources.len();
    let mut subjects = Vec::new();
    if authors > 0 {
        subjects.push(format!("{authors} author(s)"));
    }
    if journals > 0 {
        subjects.push(format!("{journals} journal(s)"));
    }
    if curated_sources > 0 {
        subjects.push(format!("{curated_sources} curated source(s)"));
    }
    let subject = subjects.join(" and ");

    let title = format!("Recent publications ({subject})");
    let description =
        format!("An RSS feed of recent publications for {subject}, parsed from {provider_name}.");
    (title, description)
}

async fn fetch_works(request: &FeedRequest) -> Result<Vec<Work>, String> {
    let (provider_works, curated_works) = tokio::join!(
        fetch_provider_works(request),
        curated::fetch_sources(&CLIENT, &request.curated_sources, &request.from),
    );
    Ok(merge_works(provider_works?, curated_works?))
}

async fn fetch_provider_works(request: &FeedRequest) -> Result<Vec<Work>, String> {
    match request.provider {
        Provider::OpenAlex => fetch_openalex_works(request).await,
        Provider::GoogleScholar => {
            let works = google_scholar::fetch_works(
                &CLIENT,
                &request.google_scholar_authors,
                &request.from,
            )
            .await?;
            Ok(works)
        }
    }
}

/// Fetch works for the feed as the UNION of an author query and a journal query,
/// merged, deduplicated by work id, and sorted newest-first.
async fn fetch_openalex_works(request: &FeedRequest) -> Result<Vec<Work>, String> {
    let topic_suffix = if request.topics.is_empty() {
        String::new()
    } else {
        format!(",topics.id:{}", request.topics.join("|"))
    };
    let from_suffix = format!(",from_publication_date:{}", request.from);

    let author_filter = (!request.author_ids.is_empty()).then(|| {
        format!(
            "author.id:{}{from_suffix}{topic_suffix}",
            request.author_ids.join("|")
        )
    });
    let journal_filter = (!request.source_ids.is_empty()).then(|| {
        format!(
            "primary_location.source.id:{}{from_suffix}{topic_suffix}",
            request.source_ids.join("|")
        )
    });

    // Run the (up to two) queries concurrently.
    let (author_works, journal_works) = tokio::join!(
        fetch_works_for_filter(author_filter),
        fetch_works_for_filter(journal_filter),
    );
    let mut author_works = author_works?;
    let mut journal_works = journal_works?;
    mark_feed_authors(&mut author_works, &request.author_ids);
    mark_feed_authors(&mut journal_works, &request.author_ids);

    Ok(merge_works(author_works, journal_works))
}

fn mark_feed_authors(works: &mut [Work], feed_author_ids: &[String]) {
    for work in works {
        let Some(authorships) = work.authorships.as_deref() else {
            continue;
        };

        for authorship in authorships {
            let Some(author) = authorship.author.as_ref() else {
                continue;
            };
            let Some(author_id) = author.id.as_deref().map(normalize_id) else {
                continue;
            };
            if !feed_author_ids.contains(&author_id) {
                continue;
            }

            work.matched_author_names
                .extend(author.display_name.iter().cloned());
            work.matched_author_names
                .extend(authorship.raw_author_name.iter().cloned());
        }
        work.matched_author_names.sort();
        work.matched_author_names.dedup();
    }
}

/// A signature that identifies a work well enough to treat two records as versions of
/// each other. Two works are grouped when *any* of their keys match.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum VersionKey {
    /// Identical DOIs always denote the same work.
    Doi(String),
    /// Normalized title plus the complete set of OpenAlex author ids.
    AuthorIds(String, Vec<String>),
    /// Normalized title plus the complete set of normalized author names. Catches the
    /// common case where OpenAlex minted two author entities for the same person.
    AuthorNames(String, Vec<String>),
}

/// Merge two result sets, deduplicating exact OpenAlex records and grouping versions that
/// share a DOI, or a normalized title together with the same author ids *or* author names.
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
                // Register the incoming record's keys too, so later records matching
                // either variant land in the same group.
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

    works.sort_by(|a, b| {
        let da = a.publication_date.as_deref().unwrap_or("");
        let db = b.publication_date.as_deref().unwrap_or("");
        db.cmp(da)
    });

    works
}

pub(crate) fn version_keys(work: &Work) -> Vec<VersionKey> {
    let mut keys = Vec::new();

    if let Some(doi) = work.doi.as_deref().map(normalize_doi) {
        if !doi.is_empty() {
            keys.push(VersionKey::Doi(doi));
        }
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

    // Only usable when *every* authorship carries an id: a partial set would let two
    // genuinely different works collide.
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

    // Author names come in two flavours that disagree surprisingly often (`F Vermeire`
    // vs the raw `Florence Vermeire`), so key on each variant independently.
    let name_sources: [fn(&crate::openalex::Authorship) -> Option<&str>; 3] = [
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

/// Reduce a DOI to a comparable form: bare lowercase `10.x/y` without the resolver prefix.
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

/// Normalize an author name into an order-independent set of name tokens, so that
/// `Tang, Sophia`, `Sophia Tang` and `Sophia  TANG` all compare equal. Single-character
/// initials are dropped so `S. Tang` also matches, unless that would leave nothing.
fn normalize_author_name(name: &str) -> String {
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
    existing.matched_author_names = matched_author_names;
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

/// Run a single `/works` query for the given filter (or return empty if `None`).
async fn fetch_works_for_filter(filter: Option<String>) -> Result<Vec<Work>, String> {
    let filter = match filter {
        Some(f) => f,
        None => return Ok(Vec::new()),
    };

    let mut pairs = vec![
        ("filter".to_string(), filter),
        ("sort".to_string(), "publication_date:desc".to_string()),
        ("per_page".to_string(), "50".to_string()),
    ];
    if let Some(m) = mailto() {
        pairs.push(("mailto".to_string(), m));
    }

    let url = url::Url::parse_with_params(&format!("{API_BASE}/works"), &pairs)
        .map_err(|error| format!("Failed to build OpenAlex URL: {error}"))?;
    let response = CLIENT
        .get(url)
        .send()
        .await
        .map_err(|error| format!("OpenAlex request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("OpenAlex request failed: {error}"))?;
    let body = response
        .json::<WorksResponse>()
        .await
        .map_err(|error| format!("Failed to parse OpenAlex response: {error}"))?;

    Ok(body.results)
}

fn work_to_item(work: &Work) -> rss::Item {
    let link = work.best_link();

    let guid = link
        .clone()
        .map(|value| GuidBuilder::default().value(value).permalink(true).build());

    let source = work.venue().map(|name| Source {
        url: work
            .best_link()
            .unwrap_or_else(|| String::from("https://openalex.org")),
        title: Some(name),
    });

    let mut description = match (work.venue(), work.cited_by_count) {
        (Some(venue), Some(cites)) if cites > 0 => Some(format!("{venue} — cited {cites} times")),
        (Some(venue), _) => Some(venue),
        (None, Some(cites)) if cites > 0 => Some(format!("Cited {cites} times")),
        (None, _) => None,
    };

    if !work.alternate_links.is_empty() {
        let label = if work.alternate_links.len() == 1 {
            "Alternate version"
        } else {
            "Alternate versions"
        };
        let alternates = format!("{label}: {}", work.alternate_links.join(", "));
        description = Some(match description {
            Some(current) => format!("{current}\n{alternates}"),
            None => alternates,
        });
    }

    if let Some(pdf_url) = work.oa_pdf_url() {
        let pdf = format!("Open-access PDF: {pdf_url}");
        description = Some(match description {
            Some(current) => format!("{current}\n{pdf}"),
            None => pdf,
        });
    }

    if !work.curated_sources.is_empty() {
        let sources = work
            .curated_sources
            .iter()
            .map(|source| format!("{} ({})", source.name, source.url))
            .collect::<Vec<_>>()
            .join(", ");
        let provenance = format!("Curated by: {sources}");
        description = Some(match description {
            Some(current) => format!("{current}\n{provenance}"),
            None => provenance,
        });
    }

    let author_names = work.author_names();
    let dublin_core = (!author_names.is_empty()).then(|| DublinCoreExtension {
        creators: author_names,
        ..DublinCoreExtension::default()
    });
    let category_domain = work
        .curated_sources
        .first()
        .map(|source| source.url.clone());
    let categories = work
        .curated_categories
        .iter()
        .map(|name| Category {
            name: name.clone(),
            domain: category_domain.clone(),
        })
        .collect::<Vec<_>>();

    ItemBuilder::default()
        .title(Some(work.best_title()))
        .description(description)
        .link(link)
        .guid(guid)
        .source(source)
        .pub_date(work.publication_date.as_deref().and_then(to_rfc2822))
        .dublin_core_ext(dublin_core)
        .categories(categories)
        .content(work.abstract_text())
        .build()
}

fn work_to_publication(work: &Work) -> reader::Publication {
    let matched_names = work
        .matched_author_names
        .iter()
        .map(|name| normalize_author_name(name))
        .collect::<Vec<_>>();
    let authors = work
        .authorships
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|authorship| {
            let display_name = authorship
                .author
                .as_ref()
                .and_then(|author| author.display_name.clone())
                .or_else(|| authorship.raw_author_name.clone())?;
            let matched_feed = authorship
                .author
                .as_ref()
                .and_then(|author| author.display_name.as_deref())
                .into_iter()
                .chain(authorship.raw_author_name.as_deref())
                .map(normalize_author_name)
                .any(|name| matched_names.contains(&name));

            Some(reader::Author {
                name: display_name,
                matched_feed,
            })
        })
        .collect();

    reader::Publication {
        id: work.id.clone().or_else(|| work.best_link()),
        title: work.best_title(),
        link: work.best_link(),
        publication_date: work.publication_date.clone(),
        venue: work.venue(),
        authors,
        abstract_text: work.abstract_text(),
        curated_sources: work
            .curated_sources
            .iter()
            .map(|source| reader::Attribution {
                name: source.name.clone(),
                url: source.url.clone(),
            })
            .collect(),
        curated_categories: work.curated_categories.clone(),
    }
}

/// Convert an OpenAlex `YYYY-MM-DD` date into an RFC-2822 timestamp.
fn to_rfc2822(date: &str) -> Option<String> {
    let naive = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let datetime = naive.and_time(NaiveTime::from_hms_opt(0, 0, 0)?);
    let utc = chrono::DateTime::<Utc>::from_naive_utc_and_offset(datetime, Utc);
    Some(utc.to_rfc2822())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(id: Option<&str>, date: Option<&str>) -> Work {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "publication_date": date,
        }))
        .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn version_work(
        id: &str,
        doi: &str,
        title: &str,
        date: &str,
        author_ids: &[&str],
        version: &str,
        pdf_url: Option<&str>,
        has_abstract: bool,
    ) -> Work {
        let authorships = author_ids
            .iter()
            .map(|id| serde_json::json!({"author": {"id": id}}))
            .collect::<Vec<_>>();
        let best_oa_location = pdf_url.map(|url| {
            serde_json::json!({
                "pdf_url": url,
                "version": version
            })
        });
        let abstract_index = has_abstract.then(|| serde_json::json!({"Abstract": [0]}));

        serde_json::from_value(serde_json::json!({
            "id": id,
            "doi": doi,
            "title": title,
            "publication_date": date,
            "authorships": authorships,
            "primary_location": {
                "landing_page_url": doi,
                "version": version
            },
            "best_oa_location": best_oa_location,
            "abstract_inverted_index": abstract_index
        }))
        .unwrap()
    }

    fn ids(works: &[Work]) -> Vec<Option<String>> {
        works.iter().map(|w| w.id.clone()).collect()
    }

    #[test]
    fn merge_dedupes_by_id_across_both_sets() {
        let primary = vec![
            work(Some("W1"), Some("2025-01-01")),
            work(Some("W2"), Some("2025-03-01")),
        ];
        let secondary = vec![
            work(Some("W2"), Some("2025-03-01")),
            work(Some("W3"), Some("2025-02-01")),
        ];

        let merged = merge_works(primary, secondary);

        // W2 appears once; result sorted newest-first: W2 (03), W3 (02), W1 (01).
        assert_eq!(
            ids(&merged),
            vec![
                Some("W2".to_string()),
                Some("W3".to_string()),
                Some("W1".to_string())
            ]
        );
    }

    #[test]
    fn merge_unions_matched_authors_for_identical_provider_results() {
        let mut first = work(Some("W1"), Some("2025-01-01"));
        first.matched_author_names = vec!["Ada Lovelace".to_string()];
        let mut second = work(Some("W1"), Some("2025-01-01"));
        second.matched_author_names = vec!["Grace Hopper".to_string()];

        let merged = merge_works(vec![first], vec![second]);

        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].matched_author_names,
            vec!["Ada Lovelace", "Grace Hopper"]
        );
    }

    #[test]
    fn merge_keeps_items_without_id_and_puts_missing_dates_last() {
        let primary = vec![work(None, Some("2025-05-01")), work(None, None)];
        let secondary = vec![work(Some("W9"), None)];

        let merged = merge_works(primary, secondary);

        assert_eq!(merged.len(), 3);
        // The dated item comes first; the two date-less items follow.
        assert_eq!(merged[0].publication_date.as_deref(), Some("2025-05-01"));
        assert!(merged[1].publication_date.is_none());
        assert!(merged[2].publication_date.is_none());
    }

    #[test]
    fn merge_groups_versions_and_keeps_the_richest_accessible_record() {
        let published = version_work(
            "W-published",
            "https://doi.org/10.1109/example",
            "DNAS-Bench: Deterministic Nucleic Acid Screener Benchmarking",
            "2026-05-21",
            &["A1", "A2", "A3"],
            "publishedVersion",
            None,
            false,
        );
        let preprint = version_work(
            "W-preprint",
            "https://doi.org/10.64898/example",
            "DNAS Bench — Deterministic Nucleic Acid Screener Benchmarking",
            "2026-07-20",
            &["A3", "A1", "A2"],
            "acceptedVersion",
            Some("https://example.com/preprint.pdf"),
            true,
        );

        let merged = merge_works(vec![published], vec![preprint]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id.as_deref(), Some("W-preprint"));
        assert_eq!(
            merged[0].alternate_links,
            vec!["https://doi.org/10.1109/example"]
        );
        assert!(work_to_item(&merged[0])
            .description
            .as_deref()
            .is_some_and(|description| description.contains("Alternate version:")));
    }

    #[test]
    fn merge_keeps_same_title_with_different_authors_separate() {
        let first = version_work(
            "W1",
            "https://doi.org/10.example/one",
            "Shared title",
            "2026-05-21",
            &["A1", "A2"],
            "publishedVersion",
            None,
            false,
        );
        let second = version_work(
            "W2",
            "https://doi.org/10.example/two",
            "Shared title",
            "2026-05-22",
            &["A1", "A3"],
            "publishedVersion",
            None,
            false,
        );

        assert_eq!(merge_works(vec![first], vec![second]).len(), 2);
    }

    #[test]
    fn merge_groups_duplicate_author_entities_with_same_title() {
        // OpenAlex sometimes mints two author ids for the same person, which used to
        // defeat the author-id-only version key ("Expanding Flow Maps").
        let first: Work = serde_json::from_value(serde_json::json!({
            "id": "https://openalex.org/W7171268167",
            "title": "Expanding Flow Maps",
            "publication_date": "2026-07-23",
            "authorships": [
                {"author": {"id": "https://openalex.org/A5143604557", "display_name": "Sophia Tang"}},
                {"author": {"id": "https://openalex.org/A5016342562", "display_name": "Pranam Chatterjee"}}
            ]
        }))
        .unwrap();
        let second: Work = serde_json::from_value(serde_json::json!({
            "id": "https://openalex.org/W7170527570",
            "doi": "https://doi.org/10.48550/arxiv.2607.21585",
            "title": "Expanding Flow Maps",
            "publication_date": "2026-07-23",
            "authorships": [
                {"author": {"id": "https://openalex.org/A5143533688"}, "raw_author_name": "Tang, Sophia"},
                {"author": {"id": "https://openalex.org/A5016342562", "display_name": "Pranam Chatterjee"}}
            ]
        }))
        .unwrap();

        assert_eq!(merge_works(vec![first], vec![second]).len(), 1);
    }

    #[test]
    fn merge_groups_records_sharing_a_doi() {
        let first: Work = serde_json::from_value(serde_json::json!({
            "id": "W1",
            "doi": "https://doi.org/10.1234/Example",
            "title": "One title"
        }))
        .unwrap();
        let second: Work = serde_json::from_value(serde_json::json!({
            "id": "W2",
            "doi": "doi:10.1234/example",
            "title": "A differently recorded title"
        }))
        .unwrap();

        assert_eq!(merge_works(vec![first], vec![second]).len(), 1);
    }

    #[test]
    fn merge_groups_versions_when_only_raw_author_names_agree() {
        // chemRxiv v1/v2: display names diverge (`F Vermeire` vs `Florence H. Vermeire`)
        // while the raw names recorded on the authorship agree.
        let v1: Work = serde_json::from_value(serde_json::json!({
            "id": "W-v1",
            "doi": "https://doi.org/10.26434/chemrxiv.15003516/v1",
            "title": "QuantumPioneer: Scalable generation of quantum chemical data",
            "publication_date": "2026-05-18",
            "authorships": [
                {"author": {"id": "A5090585248", "display_name": "F Vermeire"}, "raw_author_name": "Florence Vermeire"},
                {"author": {"id": "A5051475129", "display_name": "William H. Green"}, "raw_author_name": "William H. Green"}
            ]
        }))
        .unwrap();
        let v2: Work = serde_json::from_value(serde_json::json!({
            "id": "W-v2",
            "doi": "https://doi.org/10.26434/chemrxiv.15003516/v2",
            "title": "QuantumPioneer: Scalable generation of quantum chemical data",
            "publication_date": "2026-07-12",
            "authorships": [
                {"author": {"id": "A5022062241", "display_name": "Florence H. Vermeire"}, "raw_author_name": "Florence Vermeire"},
                {"author": {"id": "A5051475129", "display_name": "William H. Green"}, "raw_author_name": "William H. Green"}
            ]
        }))
        .unwrap();

        assert_eq!(merge_works(vec![v1], vec![v2]).len(), 1);
    }

    #[test]
    fn merge_keeps_same_title_with_different_author_names_separate() {
        let first: Work = serde_json::from_value(serde_json::json!({
            "id": "W1",
            "title": "Shared title",
            "authorships": [{"author": {"display_name": "Ada Lovelace"}}]
        }))
        .unwrap();
        let second: Work = serde_json::from_value(serde_json::json!({
            "id": "W2",
            "title": "Shared title",
            "authorships": [{"author": {"display_name": "Grace Hopper"}}]
        }))
        .unwrap();

        assert_eq!(merge_works(vec![first], vec![second]).len(), 2);
    }

    #[test]
    fn author_names_normalize_order_case_and_initials() {
        assert_eq!(
            normalize_author_name("Tang, Sophia"),
            normalize_author_name("Sophia  TANG")
        );
        assert_eq!(
            normalize_author_name("S. Tang"),
            normalize_author_name("Tang")
        );
        assert_ne!(
            normalize_author_name("Ada Lovelace"),
            normalize_author_name("Grace Hopper")
        );
    }

    #[test]
    fn item_uses_dublin_core_creators_and_omits_unknown_length_enclosure() {
        let work: Work = serde_json::from_value(serde_json::json!({
            "id": "https://openalex.org/W1",
            "title": "Example",
            "authorships": [
                {"author": {"display_name": "Ada Lovelace"}},
                {"author": {}, "raw_author_name": "Grace Hopper"}
            ],
            "best_oa_location": {
                "pdf_url": "https://example.com/paper.pdf"
            }
        }))
        .unwrap();

        let item = work_to_item(&work);

        assert!(item.author.is_none());
        assert!(item.enclosure.is_none());
        assert_eq!(
            item.dublin_core_ext.unwrap().creators,
            vec!["Ada Lovelace", "Grace Hopper"]
        );
        assert!(item
            .description
            .as_deref()
            .is_some_and(|description| description.contains("https://example.com/paper.pdf")));
    }

    #[test]
    fn publication_marks_every_matching_author_without_changing_rss_categories() {
        let mut works: Vec<Work> = vec![serde_json::from_value(serde_json::json!({
            "id": "https://openalex.org/W1",
            "title": "Collaborative work",
            "authorships": [
                {"author": {"id": "https://openalex.org/A1", "display_name": "Ada Lovelace"}},
                {"author": {"id": "https://openalex.org/A2"}, "raw_author_name": "Grace Hopper"},
                {"author": {"id": "https://openalex.org/A3", "display_name": "Alan Turing"}}
            ]
        }))
        .unwrap()];
        let feed_author_ids = vec!["A1".to_string(), "A2".to_string()];

        mark_feed_authors(&mut works, &feed_author_ids);
        let publication = work_to_publication(&works[0]);
        let item = work_to_item(&works[0]);

        assert_eq!(
            publication
                .authors
                .iter()
                .filter(|author| author.matched_feed)
                .map(|author| author.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Ada Lovelace", "Grace Hopper"],
        );
        assert!(item.categories.is_empty());
    }

    #[test]
    fn merge_preserves_matched_author_from_a_discarded_version() {
        let first: Work = serde_json::from_value(serde_json::json!({
            "id": "W-original",
            "title": "A shared title",
            "authorships": [
                {"author": {"id": "A-configured", "display_name": "Sophia Tang"}}
            ]
        }))
        .unwrap();
        let richer: Work = serde_json::from_value(serde_json::json!({
            "id": "W-richer",
            "title": "A shared title",
            "authorships": [
                {"author": {"id": "A-duplicate"}, "raw_author_name": "Tang, Sophia"}
            ],
            "abstract_inverted_index": {"Abstract": [0]}
        }))
        .unwrap();
        let mut first_version = vec![first];

        mark_feed_authors(&mut first_version, &["A-configured".to_string()]);
        let merged = merge_works(first_version, vec![richer]);
        let publication = work_to_publication(&merged[0]);

        assert_eq!(merged[0].id.as_deref(), Some("W-richer"));
        assert_eq!(publication.authors.len(), 1);
        assert_eq!(publication.authors[0].name, "Tang, Sophia");
        assert!(publication.authors[0].matched_feed);
    }

    #[test]
    fn provider_names_parse_with_openalex_as_the_default_spelling() {
        assert_eq!(Provider::parse("openalex").unwrap(), Provider::OpenAlex);
        assert_eq!(
            Provider::parse("google-scholar").unwrap(),
            Provider::GoogleScholar
        );
        assert_eq!(
            Provider::parse("google_scholar").unwrap(),
            Provider::GoogleScholar
        );
        assert!(Provider::parse("unknown").is_err());
    }

    #[test]
    fn parses_curated_comparison_cli_mode() {
        let cli = parse_cli_args_from(
            [
                "--compare-curated",
                "bioml",
                "peldom-protein-design",
                "--config",
                "custom.toml",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();
        let comparison = cli.comparison.unwrap();

        assert_eq!(comparison.feed_name, "bioml");
        assert_eq!(comparison.source_name, "peldom-protein-design");
        assert_eq!(cli.config_path, PathBuf::from("custom.toml"));
    }

    #[test]
    fn rejects_incomplete_curated_comparison_cli_mode() {
        let error = parse_cli_args_from(
            ["--compare-curated", "bioml"]
                .into_iter()
                .map(str::to_string),
        )
        .err()
        .unwrap();

        assert!(error.contains("requires a feed name and curated source"));
    }

    #[tokio::test]
    async fn unified_people_resolve_for_google_scholar_without_openalex_calls() {
        let config: Config = toml::from_str(
            r#"
default_feed = "bioml"

[settings]
from_days = 365

[feeds.bioml]
title = "BioML"
google_scholar_authors = ["Legacy Author"]

[[feeds.bioml.people]]
name = "Pranam Chatterjee"
openalex_id = "A1"

[[feeds.bioml.people]]
name = "David Baker"
openalex_id = "A2"
google_scholar_name = "David W Baker"
"#,
        )
        .unwrap();

        let request = resolve_feed_request(&[], &config, Provider::GoogleScholar)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(request.provider, Provider::GoogleScholar);
        assert!(request.author_ids.is_empty());
        assert_eq!(
            request.google_scholar_authors,
            vec!["David W Baker", "Legacy Author", "Pranam Chatterjee"]
        );
        assert!(request.source_ids.is_empty());
    }

    #[tokio::test]
    async fn unified_people_resolve_precise_openalex_ids() {
        let config: Config = toml::from_str(
            r#"
default_feed = "bioml"

[feeds.bioml]
title = "BioML"

[[feeds.bioml.people]]
name = "Pranam Chatterjee"
openalex_id = "https://openalex.org/A5016342562"

[[feeds.bioml.people]]
name = "Scholar only"
"#,
        )
        .unwrap();

        let request = resolve_feed_request(&[], &config, Provider::OpenAlex)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(request.provider, Provider::OpenAlex);
        assert_eq!(request.author_ids, vec!["A5016342562"]);
        assert!(request.google_scholar_authors.is_empty());
    }

    #[tokio::test]
    async fn curated_only_archive_resolves_without_provider_authors() {
        let config: Config = toml::from_str(
            r#"
[feeds.protein-design-archive]
title = "Deep Learning for Protein Design"
from = "1900-01-01"
curated_sources = ["peldom-protein-design"]
"#,
        )
        .unwrap();
        let params = vec![("feed".to_string(), "protein-design-archive".to_string())];

        let request = resolve_feed_request(&params, &config, Provider::OpenAlex)
            .await
            .unwrap()
            .unwrap();

        assert!(request.author_ids.is_empty());
        assert!(request.source_ids.is_empty());
        assert_eq!(request.curated_sources, vec!["peldom-protein-design"]);
        assert_eq!(request.from, "1900-01-01");
    }
}
