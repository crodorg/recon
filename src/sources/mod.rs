//! Free-source + external-API connectors. Each module exposes the uniform
//! `pub async fn search(client: &Client, query: &str, limit: usize) -> Result<Vec<Candidate>>`.
//! Perplexity is the exception: it takes extra filter/extraction options, so its
//! entry point is `perplexity::search_with(client, query, limit, &SearchOpts)`.

pub mod hn;
pub mod github;
pub mod polymarket;
pub mod grok;
pub mod perplexity;
pub mod openalex;
pub mod crossref;
