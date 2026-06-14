//! Perplexity Search API connector.
//!
//! Hits the SEARCH API (`POST https://api.perplexity.ai/search`) — NOT the Sonar
//! chat-completions endpoint and NOT the Agent API. The Search API returns raw,
//! ranked web results (`results[]` of `{title, url, snippet, date, last_updated}`)
//! rather than an LLM-written answer.
//!
//! Doc URL coded against: https://docs.perplexity.ai/api-reference/search-post
//! (request: `query`, `max_results` 1..=20, optional `search_domain_filter`,
//!  date/recency filters, `max_tokens_per_page`; response:
//!  `{ results: [ { title, url, snippet, date, last_updated } ], id }`).
//!
//! Verified live 2026-06-08 (Phase 3 step 0): `search_domain_filter` (max 20),
//! `search_after_date_filter`/`search_before_date_filter`/`last_updated_*` (MM/DD/YYYY),
//! `search_recency_filter` (hour|day|week|month|year), and `max_tokens_per_page` are
//! all honored. Pricing is per-request only ("no token costs"), so a large
//! `max_tokens_per_page` is FREE. It has no separately-named content field — it
//! lengthens `snippet` (probe: 256 → ~1.0-1.3k chars, 4096 → ~1.8-3.6k chars), so the
//! excerpt that feeds triage + shallow secondary evidence lands in `snippet`.

use crate::model::{Candidate, SourceType};
use reqwest::Client;
use serde::Deserialize;

const SEARCH_URL: &str = "https://api.perplexity.ai/search";

/// Per-call Perplexity Search options. The other connectors share the plain
/// `(client, query, limit)` shape; only Perplexity takes these, so they live in
/// an options struct passed to [`search_with`] rather than widening the trait.
#[derive(Clone, Debug, Default)]
pub struct SearchOpts {
    /// Restrict results to these domains (Search API caps at 20; we truncate).
    pub domains: Vec<String>,
    /// `search_after_date_filter` — results published after this `MM/DD/YYYY`.
    pub after: Option<String>,
    /// `search_before_date_filter` — results published before this `MM/DD/YYYY`.
    pub before: Option<String>,
    /// `search_recency_filter` — one of hour|day|week|month|year.
    pub recency: Option<String>,
    /// `max_tokens_per_page` — how much page content to extract into `snippet`.
    /// 0 means "omit the param, take the API default". Free (per-request pricing).
    pub max_tokens_per_page: usize,
}

/// One ranked hit inside the Search API `results[]` array.
///
/// The documented schema is `{ title, url, snippet, date, last_updated }`; the
/// last two are nullable. We default-fill so a result missing an optional field
/// (or any future additive field) still deserializes cleanly.
#[derive(Debug, Deserialize)]
struct ApiResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    snippet: String,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    last_updated: Option<String>,
}

/// Top-level Search API response envelope.
#[derive(Debug, Deserialize)]
struct ApiResponse {
    #[serde(default)]
    results: Vec<ApiResult>,
}

/// Search Perplexity's web index. Returns up to `limit` ranked candidates.
///
/// Requires `PERPLEXITY_API_KEY` in the environment; returns `Err` (never panics)
/// if it is unset. `limit` is clamped into the API-documented 1..=20 range.
/// `opts` adds domain/date/recency filters and the page-content length knob.
pub async fn search_with(
    client: &Client,
    query: &str,
    limit: usize,
    opts: &SearchOpts,
) -> anyhow::Result<Vec<Candidate>> {
    let api_key = std::env::var("PERPLEXITY_API_KEY")
        .map_err(|_| anyhow::anyhow!("PERPLEXITY_API_KEY not set"))?;

    // Search API documents max_results as an integer in 1..=20 (default 10).
    let max_results = limit.clamp(1, 20);
    let body = build_request_body(query, max_results, opts);

    // Verified live 2026-06-08 against POST /search: endpoint, headers, request body,
    // and response ({id, results:[{title,url,snippet,date,last_updated}]}).
    let resp = client
        .post(SEARCH_URL)
        .bearer_auth(&api_key)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("POST {SEARCH_URL}: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let detail = resp.text().await.unwrap_or_default();
        anyhow::bail!("perplexity search HTTP {status}: {detail}");
    }

    let parsed: ApiResponse = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("decode perplexity search response: {e}"))?;

    let candidates = parsed
        .results
        .into_iter()
        .enumerate()
        .filter(|(_, r)| !r.url.is_empty())
        .map(|(i, r)| {
            let ApiResult { title, url, snippet, date, last_updated } = r;
            // Live Search API responses carry `last_updated`, not `date` (verified
            // 2026-06-08 against POST /search). Prefer an explicit `date` if a future
            // response ever includes one, else fall back to `last_updated`.
            let effective_date = date.or_else(|| last_updated.clone());
            // No explicit score/rank field in the response, so record the 1-based
            // ranked position as `rank` plus the raw `last_updated` for provenance.
            let extra = serde_json::json!({
                "rank": i + 1,
                "last_updated": last_updated,
            });
            Candidate {
                raw_url: url,
                title,
                snippet,
                date: effective_date,
                source_type: SourceType::Web,
                origin: "perplexity".to_string(),
                extra,
            }
        })
        .collect();

    Ok(candidates)
}

/// Build the `POST /search` request body from the query, clamped result count,
/// and options. Pure (no I/O) so the param wiring is unit-testable: optional
/// filters are inserted only when set, the domain list truncates to the API's
/// 20-domain cap, and `max_tokens_per_page == 0` omits the field (API default).
fn build_request_body(query: &str, max_results: usize, opts: &SearchOpts) -> serde_json::Value {
    let mut body = serde_json::json!({
        "query": query,
        "max_results": max_results,
    });
    let obj = body.as_object_mut().expect("json object literal");
    if !opts.domains.is_empty() {
        let domains: Vec<&String> = opts.domains.iter().take(20).collect();
        obj.insert("search_domain_filter".into(), serde_json::json!(domains));
    }
    if let Some(after) = &opts.after {
        obj.insert("search_after_date_filter".into(), serde_json::json!(after));
    }
    if let Some(before) = &opts.before {
        obj.insert("search_before_date_filter".into(), serde_json::json!(before));
    }
    if let Some(recency) = &opts.recency {
        obj.insert("search_recency_filter".into(), serde_json::json!(recency));
    }
    if opts.max_tokens_per_page > 0 {
        obj.insert(
            "max_tokens_per_page".into(),
            serde_json::json!(opts.max_tokens_per_page),
        );
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_body_has_only_query_and_results() {
        let opts = SearchOpts::default();
        // default max_tokens_per_page is 0 here (the CLI defaults it to 1024; the
        // struct default is 0 = "omit"), so no optional fields should appear.
        let body = build_request_body("hello", 10, &opts);
        let obj = body.as_object().unwrap();
        assert_eq!(obj.len(), 2, "only query + max_results expected");
        assert_eq!(obj["query"], "hello");
        assert_eq!(obj["max_results"], 10);
    }

    #[test]
    fn all_filters_are_inserted_when_set() {
        let opts = SearchOpts {
            domains: vec!["sec.gov".into(), "ecfr.gov".into()],
            after: Some("01/01/2025".into()),
            before: Some("12/31/2025".into()),
            recency: Some("month".into()),
            max_tokens_per_page: 4096,
        };
        let body = build_request_body("q", 20, &opts);
        let obj = body.as_object().unwrap();
        assert_eq!(
            obj["search_domain_filter"],
            serde_json::json!(["sec.gov", "ecfr.gov"])
        );
        assert_eq!(obj["search_after_date_filter"], "01/01/2025");
        assert_eq!(obj["search_before_date_filter"], "12/31/2025");
        assert_eq!(obj["search_recency_filter"], "month");
        assert_eq!(obj["max_tokens_per_page"], 4096);
    }

    #[test]
    fn domain_filter_truncates_to_twenty() {
        let opts = SearchOpts {
            domains: (0..25).map(|i| format!("d{i}.com")).collect(),
            ..Default::default()
        };
        let body = build_request_body("q", 10, &opts);
        let domains = body["search_domain_filter"].as_array().unwrap();
        assert_eq!(domains.len(), 20, "API caps domain filter at 20");
    }

    #[test]
    fn zero_max_tokens_per_page_is_omitted() {
        let opts = SearchOpts {
            max_tokens_per_page: 0,
            ..Default::default()
        };
        let body = build_request_body("q", 10, &opts);
        assert!(body.as_object().unwrap().get("max_tokens_per_page").is_none());
    }
}
