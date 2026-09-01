mod filters;
mod render;

pub use render::{
    article_id_from_path, render_article, render_feed, FAVICON, READER_CSS, READER_JS,
};

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
    pub pdf_url: Option<String>,
    pub publication_date: Option<String>,
    pub collection_date: Option<CollectionDate>,
    pub venue: Option<String>,
    pub authors: Vec<Author>,
    pub abstract_text: Option<String>,
    pub provider_match: bool,
    pub curated_sources: Vec<Attribution>,
    pub curated_categories: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Author {
    pub name: String,
    pub filter_id: String,
    pub matched_feed: bool,
}

#[derive(Clone, Debug)]
pub struct Attribution {
    pub key: Option<String>,
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct CollectionDate {
    pub date: String,
    pub commit_url: String,
}
