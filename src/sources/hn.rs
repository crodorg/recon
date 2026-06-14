//! Hacker News connector via the Algolia search API.
//!
//! Endpoint: `GET https://hn.algolia.com/api/v1/search?query=<q>&tags=story&hitsPerPage=<n>`.
//! Each `hit` is mapped onto a [`Candidate`]. Confirmed field shapes against the
//! live API: `url` may be absent/null (Ask-HN/text posts) — we then fall back to
//! the canonical item permalink; `points`/`num_comments` are numbers;
//! `objectID`/`author` are strings; story bodies live in `story_text` (and, when
//! comments are returned, `comment_text`).

use crate::http;
use crate::model::{Candidate, SourceType};
use reqwest::Client;
use serde_json::{json, Value};

/// Max characters kept from a story/comment body for the snippet.
const SNIPPET_LEN: usize = 200;

pub async fn search(client: &Client, query: &str, limit: usize) -> anyhow::Result<Vec<Candidate>> {
    let url = build_url(query, limit);
    let body = http::get_json(client, &url).await?;

    let hits = body
        .get("hits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let candidates = hits.iter().map(hit_to_candidate).collect();
    Ok(candidates)
}

/// Build the Algolia search URL with a percent-encoded query and bounded page size.
fn build_url(query: &str, limit: usize) -> String {
    let q = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    // Algolia rejects hitsPerPage of 0; clamp to at least 1.
    let per_page = limit.max(1);
    format!("https://hn.algolia.com/api/v1/search?query={q}&tags=story&hitsPerPage={per_page}")
}

fn hit_to_candidate(hit: &Value) -> Candidate {
    let object_id = str_field(hit, "objectID").unwrap_or_default();

    // raw_url: prefer the story's external url; else the HN item permalink.
    let raw_url = str_field(hit, "url")
        .unwrap_or_else(|| format!("https://news.ycombinator.com/item?id={object_id}"));

    let title = str_field(hit, "title").unwrap_or_default();

    // snippet: first ~200 chars of story_text, then comment_text, else empty.
    let snippet = str_field(hit, "story_text")
        .or_else(|| str_field(hit, "comment_text"))
        .map(|s| truncate_chars(&s, SNIPPET_LEN))
        .unwrap_or_default();

    let date = str_field(hit, "created_at");

    let extra = json!({
        "points": hit.get("points").cloned().unwrap_or(Value::Null),
        "num_comments": hit.get("num_comments").cloned().unwrap_or(Value::Null),
        "objectID": hit.get("objectID").cloned().unwrap_or(Value::Null),
        "author": hit.get("author").cloned().unwrap_or(Value::Null),
    });

    Candidate {
        raw_url,
        title,
        snippet,
        date,
        source_type: SourceType::News,
        origin: "hackernews".to_string(),
        extra,
    }
}

/// Pull a non-empty string field from a JSON object; `None` if absent, null, or
/// not a string.
fn str_field(hit: &Value, key: &str) -> Option<String> {
    match hit.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    }
}

/// Truncate to at most `max` characters (not bytes), respecting char boundaries.
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}
