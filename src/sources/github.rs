//! GitHub repository-search connector.
//!
//! Maps `GET /search/repositories?q=...&sort=stars&order=desc&per_page=<limit>`
//! into normalized [`Candidate`]s. A `User-Agent` is mandatory for the GitHub
//! API; the shared `http::client()` already sets one. If `GITHUB_TOKEN` is set
//! we send `Authorization: Bearer <token>` to lift the rate limit (60 -> 5000
//! requests/hour for search-adjacent unauthenticated vs. authenticated calls).
//!
//! Rate limiting (HTTP 403/429 with `x-ratelimit-remaining: 0`) is reported as a
//! clear `Err` rather than surfacing as an opaque decode failure.

use crate::model::{Candidate, SourceType};
use reqwest::Client;
use serde_json::json;

const API: &str = "https://api.github.com/search/repositories";

pub async fn search(client: &Client, query: &str, limit: usize) -> anyhow::Result<Vec<Candidate>> {
    // GitHub rejects per_page=0 and caps it at 100.
    let per_page = limit.clamp(1, 100);
    let url = format!(
        "{API}?q={}&sort=stars&order=desc&per_page={per_page}",
        urlencode(query)
    );

    let mut req = client
        .get(&url)
        // Pin the documented media type so the response shape is stable.
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");

    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let token = token.trim();
        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("github: request failed: {e}"))?;

    let status = resp.status();

    // Detect rate limiting before we attempt to decode the body. GitHub uses
    // 403 (classic) or 429 with `x-ratelimit-remaining: 0` for limit hits.
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        let remaining = header(&resp, "x-ratelimit-remaining");
        let retry_after = header(&resp, "retry-after");
        let reset = header(&resp, "x-ratelimit-reset");
        if remaining.as_deref() == Some("0") || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let hint = if std::env::var("GITHUB_TOKEN")
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false)
            {
                "authenticated limit exhausted"
            } else {
                "unauthenticated; set GITHUB_TOKEN to raise the limit"
            };
            let when = retry_after
                .map(|s| format!("retry after {s}s"))
                .or_else(|| reset.map(|s| format!("resets at unix {s}")))
                .unwrap_or_else(|| "retry later".to_string());
            anyhow::bail!("github: rate limit hit ({hint}); {when}");
        }
        // A non-rate-limit 403 (e.g. abuse detection / forbidden) — surface the body message.
        let msg = error_message(resp).await;
        anyhow::bail!("github: forbidden (403): {msg}");
    }

    if !status.is_success() {
        let code = status.as_u16();
        let msg = error_message(resp).await;
        anyhow::bail!("github: HTTP {code}: {msg}");
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("github: decode JSON: {e}"))?;

    let items = body
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        if let Some(c) = candidate_from(item) {
            out.push(c);
        }
    }
    Ok(out)
}

/// Build a `Candidate` from one repo search result. Skips items missing the
/// `html_url` we need as the canonical raw URL.
fn candidate_from(item: &serde_json::Value) -> Option<Candidate> {
    let raw_url = item.get("html_url").and_then(|v| v.as_str())?.to_string();

    let title = item
        .get("full_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let snippet = item
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Prefer last-push date, fall back to last-update.
    let date = item
        .get("pushed_at")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("updated_at").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    let stars = item.get("stargazers_count").cloned().unwrap_or(json!(null));
    let language = item.get("language").cloned().unwrap_or(json!(null));
    // `forks` and `forks_count` are aliases; prefer `forks_count`, then `forks`.
    let forks = item
        .get("forks_count")
        .or_else(|| item.get("forks"))
        .cloned()
        .unwrap_or(json!(null));

    let extra = json!({
        "stars": stars,
        "language": language,
        "forks": forks,
    });

    Some(Candidate {
        raw_url,
        title,
        snippet,
        date,
        source_type: SourceType::Code,
        origin: "github".to_string(),
        extra,
    })
}

/// Percent-encode a query string for the `q` parameter. Encodes everything that
/// is not an unreserved character; spaces become `+` (GitHub accepts both).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Read a response header as an owned string.
fn header(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Pull GitHub's `message` field out of an error body, falling back to a snippet
/// of the raw body when it is not JSON.
async fn error_message(resp: reqwest::Response) -> String {
    match resp.text().await {
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| {
                let t = text.trim();
                if t.is_empty() {
                    "(empty body)".to_string()
                } else {
                    t.chars().take(200).collect()
                }
            }),
        Err(e) => format!("(could not read body: {e})"),
    }
}
