//! OpenAlex connector — open scholarly works discovery.
//!
//! Endpoint: `GET https://api.openalex.org/works?search=<q>&per_page=<n>`.
//! `search` runs OpenAlex's full-text-ish relevance search over title/abstract/
//! fulltext and returns `results[]` of work objects.
//!
//! Auth (doc coded against `developers.openalex.org`, verified 2026-06-13): the
//! API key is the **`api_key` query param** — free, and required for non-demo use
//! since Jan 2026 (a few keyless calls still work for testing). The polite-pool
//! `mailto` is also a query param. Both are read from the environment
//! (`OPENALEX_API_KEY` / `OPENALEX_MAILTO`) and OMITTED when unset, so the public
//! crate ships no personal email and works keyless for light use.
//!
//! A work's abstract is delivered as an `abstract_inverted_index`
//! (`{word: [positions]}`) — never as plain text — so we rebuild it for the snippet.

use crate::http;
use crate::model::{Candidate, SourceType};
use reqwest::Client;
use serde_json::{json, Value};

/// Max characters kept from the rebuilt abstract for the snippet.
const SNIPPET_LEN: usize = 360;

pub async fn search(client: &Client, query: &str, limit: usize) -> anyhow::Result<Vec<Candidate>> {
    let url = build_url(query, limit);
    let body = http::get_json(client, &url).await?;

    let results = body
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let candidates = results.iter().map(work_to_candidate).collect();
    Ok(candidates)
}

/// Build the works-search URL: percent-encoded query, bounded `per_page`, and the
/// optional `mailto` / `api_key` appended only when present in the environment.
fn build_url(query: &str, limit: usize) -> String {
    let q = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    // OpenAlex per_page is 1..=200; default 25. Clamp into range.
    let per_page = limit.clamp(1, 200);
    let mut url = format!("https://api.openalex.org/works?search={q}&per_page={per_page}");
    if let Ok(mailto) = std::env::var("OPENALEX_MAILTO") {
        if !mailto.is_empty() {
            let m = url::form_urlencoded::byte_serialize(mailto.as_bytes()).collect::<String>();
            url.push_str(&format!("&mailto={m}"));
        }
    }
    if let Ok(key) = std::env::var("OPENALEX_API_KEY") {
        if !key.is_empty() {
            let k = url::form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>();
            url.push_str(&format!("&api_key={k}"));
        }
    }
    url
}

fn work_to_candidate(work: &Value) -> Candidate {
    let title = str_field(work, "display_name")
        .or_else(|| str_field(work, "title"))
        .unwrap_or_default();

    // raw_url: prefer the DOI URL (canonicalizes to `doi:…`, DOI-verified). Else
    // the best OA landing page, else the OpenAlex id.
    let doi = str_field(work, "doi"); // already a full https://doi.org/… URL
    let raw_url = doi
        .clone()
        .or_else(|| oa_landing(work))
        .or_else(|| str_field(work, "id"))
        .unwrap_or_default();

    let snippet = work
        .get("abstract_inverted_index")
        .and_then(reconstruct_abstract)
        .map(|a| truncate_chars(&a, SNIPPET_LEN))
        .unwrap_or_default();

    // Prefer the full publication_date; fall back to the bare year.
    let date = str_field(work, "publication_date").or_else(|| {
        work.get("publication_year")
            .and_then(Value::as_i64)
            .map(|y| y.to_string())
    });

    let extra = json!({
        "doi": doi,
        "oa_url": oa_url(work),
        "cited_by_count": work.get("cited_by_count").cloned().unwrap_or(Value::Null),
        "type": work.get("type").cloned().unwrap_or(Value::Null),
        "openalex_id": work.get("id").cloned().unwrap_or(Value::Null),
    });

    Candidate {
        raw_url,
        title,
        snippet,
        date,
        source_type: SourceType::Academic,
        origin: "openalex".to_string(),
        extra,
    }
}

/// Best open-access PDF/landing URL, if any (`best_oa_location` then `open_access.oa_url`).
fn oa_landing(work: &Value) -> Option<String> {
    let loc = work.get("best_oa_location")?;
    str_field(loc, "landing_page_url").or_else(|| str_field(loc, "pdf_url"))
}

fn oa_url(work: &Value) -> Value {
    work.get("best_oa_location")
        .and_then(|l| l.get("pdf_url"))
        .filter(|v| !v.is_null())
        .or_else(|| work.get("open_access").and_then(|o| o.get("oa_url")))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Rebuild plain-text abstract from OpenAlex's inverted index
/// (`{ "word": [pos, …], … }`). Words are placed at their positions and joined by
/// spaces; gaps (rare) are skipped. Returns None for an absent/empty/non-object index.
fn reconstruct_abstract(index: &Value) -> Option<String> {
    let obj = index.as_object()?;
    if obj.is_empty() {
        return None;
    }
    let mut slots: Vec<(u64, &str)> = Vec::new();
    for (word, positions) in obj {
        if let Some(arr) = positions.as_array() {
            for p in arr {
                if let Some(pos) = p.as_u64() {
                    slots.push((pos, word.as_str()));
                }
            }
        }
    }
    if slots.is_empty() {
        return None;
    }
    slots.sort_by_key(|(pos, _)| *pos);
    let text = slots
        .into_iter()
        .map(|(_, w)| w)
        .collect::<Vec<_>>()
        .join(" ");
    Some(text)
}

/// Pull a non-empty string field; None if absent, null, or empty.
fn str_field(v: &Value, key: &str) -> Option<String> {
    match v.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    }
}

/// Truncate to at most `max` characters (not bytes), respecting char boundaries.
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_has_query_and_per_page_clamped() {
        let url = build_url("crispr gene editing", 500);
        assert!(url.contains("search=crispr+gene+editing"));
        assert!(url.contains("per_page=200"), "per_page clamps to 200");
        // No env set in this test → no api_key / mailto appended.
        assert!(!url.contains("api_key="));
    }

    #[test]
    fn abstract_inverted_index_rebuilds_in_order() {
        let idx = json!({
            "Despite": [0],
            "decades": [1],
            "of": [2, 4],
            "research": [3],
            "progress": [5]
        });
        let text = reconstruct_abstract(&idx).unwrap();
        assert_eq!(text, "Despite decades of research of progress");
    }

    #[test]
    fn empty_or_missing_abstract_is_none() {
        assert!(reconstruct_abstract(&json!({})).is_none());
        assert!(reconstruct_abstract(&Value::Null).is_none());
    }

    #[test]
    fn work_prefers_doi_url_and_academic_type() {
        let work = json!({
            "display_name": "A Title",
            "doi": "https://doi.org/10.1234/abc",
            "publication_year": 2023,
            "publication_date": "2023-05-01",
            "cited_by_count": 42,
            "type": "article",
            "abstract_inverted_index": {"Hello": [0], "world": [1]}
        });
        let c = work_to_candidate(&work);
        assert_eq!(c.raw_url, "https://doi.org/10.1234/abc");
        assert_eq!(c.title, "A Title");
        assert_eq!(c.snippet, "Hello world");
        assert_eq!(c.date.as_deref(), Some("2023-05-01"));
        assert_eq!(c.source_type, SourceType::Academic);
        assert_eq!(c.origin, "openalex");
        assert_eq!(c.extra["cited_by_count"], json!(42));
    }

    #[test]
    fn work_falls_back_to_oa_landing_then_year() {
        let work = json!({
            "display_name": "No DOI",
            "publication_year": 2019,
            "best_oa_location": {"landing_page_url": "https://example.org/paper"}
        });
        let c = work_to_candidate(&work);
        assert_eq!(c.raw_url, "https://example.org/paper");
        assert_eq!(c.date.as_deref(), Some("2019"));
        assert_eq!(c.snippet, "");
    }
}
