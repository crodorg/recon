//! Free-source + external-API connectors. Each module exposes the uniform
//! `pub async fn search(client: &Client, query: &str, limit: usize) -> Result<Vec<Candidate>>`.
//! Perplexity is the exception: it takes extra filter/extraction options, so its
//! entry point is `perplexity::search_with(client, query, limit, &SearchOpts)`.

pub mod crossref;
pub mod github;
pub mod grok;
pub mod hn;
pub mod openalex;
pub mod perplexity;
pub mod polymarket;
