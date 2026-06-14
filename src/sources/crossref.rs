//! Crossref connector — DOI/metadata authority + scholarly corroborator.
//!
//! Endpoint: `GET https://api.crossref.org/works?query=<q>&rows=<n>`. Returns
//! `message.items[]` of work records. No API key (doc coded against
//! `crossref.org/documentation/retrieve-metadata/rest-api`, verified 2026-06-13):
//! "No sign-up is required." Add `mailto=<email>` for the **polite pool** (better,
//! more reliable rate limits) — read from env (`CROSSREF_MAILTO`) and OMITTED when
//! unset, so the public crate ships no personal email.
//!
//! Abstracts, when present, are JATS-XML fragments (`<jats:p>…`), so we strip tags
//! for the snippet. The DOI is the gold here — it feeds both the citation and the
//! sci-hub full-text auto-route.

use crate::http;
use crate::model::{Candidate, SourceType};
use reqwest::Client;
use serde_json::{json, Value};

/// Max characters kept from the (tag-stripped) abstract for the snippet.
const SNIPPET_LEN: usize = 360;

pub async fn search(client: &Client, query: &str, limit: usize) -> anyhow::Result<Vec<Candidate>> {
    let url = build_url(query, limit);
    let body = http::get_json(client, &url).await?;

    let items = body
        .get("message")
        .and_then(|m| m.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let candidates = items.iter().map(item_to_candidate).collect();
    Ok(candidates)
}

/// Build the works-query URL: percent-encoded query, bounded `rows`, and the
/// optional polite-pool `mailto` appended only when present in the environment.
fn build_url(query: &str, limit: usize) -> String {
    let q = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    // Crossref rows: 0..=1000; default 20. Clamp to at least 1.
    let rows = limit.clamp(1, 1000);
    let mut url = format!("https://api.crossref.org/works?query={q}&rows={rows}");
    if let Ok(mailto) = std::env::var("CROSSREF_MAILTO") {
        if !mailto.is_empty() {
            let m = url::form_urlencoded::byte_serialize(mailto.as_bytes()).collect::<String>();
            url.push_str(&format!("&mailto={m}"));
        }
    }
    url
}

fn item_to_candidate(item: &Value) -> Candidate {
    let title = first_array_str(item, "title").unwrap_or_default();

    // DOI is always present on a Crossref work; prefer the resolver URL field, else
    // synthesize from the bare DOI. Canonicalizes to `doi:…` (DOI-verified).
    let doi = str_field(item, "DOI");
    let raw_url = str_field(item, "URL")
        .or_else(|| doi.as_ref().map(|d| format!("https://doi.org/{d}")))
        .unwrap_or_default();

    let snippet = str_field(item, "abstract")
        .map(|a| truncate_chars(&strip_tags(&a), SNIPPET_LEN))
        .unwrap_or_else(|| {
            // No abstract → a compact descriptor from container + type.
            let container = first_array_str(item, "container-title").unwrap_or_default();
            let typ = str_field(item, "type").unwrap_or_default();
            [container, typ]
                .iter()
                .filter(|s| !s.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" · ")
        });

    let date = issued_date(item);

    let extra = json!({
        "doi": doi,
        "container_title": item.get("container-title").cloned().unwrap_or(Value::Null),
        "type": item.get("type").cloned().unwrap_or(Value::Null),
        "is_referenced_by_count": item.get("is-referenced-by-count").cloned().unwrap_or(Value::Null),
    });

    Candidate {
        raw_url,
        title,
        snippet,
        date,
        source_type: SourceType::Academic,
        origin: "crossref".to_string(),
        extra,
    }
}

/// Crossref dates come as `issued.date-parts` = `[[year, month?, day?]]`. Render
/// the first tuple as `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`.
fn issued_date(item: &Value) -> Option<String> {
    let parts = item
        .get("issued")
        .and_then(|i| i.get("date-parts"))
        .and_then(Value::as_array)?
        .first()
        .and_then(Value::as_array)?;
    let nums: Vec<i64> = parts.iter().filter_map(Value::as_i64).collect();
    match nums.as_slice() {
        [y] => Some(format!("{y:04}")),
        [y, m] => Some(format!("{y:04}-{m:02}")),
        [y, m, d, ..] => Some(format!("{y:04}-{m:02}-{d:02}")),
        _ => None,
    }
}

/// First non-empty string in a string-array field (Crossref `title`,
/// `container-title` are arrays).
fn first_array_str(item: &Value, key: &str) -> Option<String> {
    item.get(key)?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

/// Pull a non-empty string field; None if absent, null, or empty.
fn str_field(v: &Value, key: &str) -> Option<String> {
    match v.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    }
}

/// Strip XML/HTML tags (e.g. JATS `<jats:p>`) and collapse whitespace, so an
/// abstract fragment becomes plain snippet text.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate to at most `max` characters (not bytes), respecting char boundaries.
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_has_query_and_rows_clamped() {
        let url = build_url("machine learning", 5000);
        assert!(url.contains("query=machine+learning"));
        assert!(url.contains("rows=1000"), "rows clamps to 1000");
        assert!(!url.contains("mailto="), "no mailto without env");
    }

    #[test]
    fn strips_jats_abstract_tags() {
        let a = "<jats:p>We show <jats:italic>that</jats:italic>  X.</jats:p>";
        assert_eq!(strip_tags(a), "We show that X.");
    }

    #[test]
    fn issued_date_handles_partial_tuples() {
        let y = json!({"issued": {"date-parts": [[2021]]}});
        assert_eq!(issued_date(&y).as_deref(), Some("2021"));
        let ym = json!({"issued": {"date-parts": [[2021, 3]]}});
        assert_eq!(issued_date(&ym).as_deref(), Some("2021-03"));
        let ymd = json!({"issued": {"date-parts": [[2021, 3, 9]]}});
        assert_eq!(issued_date(&ymd).as_deref(), Some("2021-03-09"));
    }

    #[test]
    fn item_maps_to_doi_url_and_academic() {
        let item = json!({
            "title": ["Molecular Structure of Nucleic Acids"],
            "DOI": "10.1038/171737a0",
            "URL": "https://doi.org/10.1038/171737a0",
            "container-title": ["Nature"],
            "type": "journal-article",
            "is-referenced-by-count": 5000,
            "issued": {"date-parts": [[1953, 4, 25]]},
            "abstract": "<jats:p>Structure proposed.</jats:p>"
        });
        let c = item_to_candidate(&item);
        assert_eq!(c.raw_url, "https://doi.org/10.1038/171737a0");
        assert_eq!(c.title, "Molecular Structure of Nucleic Acids");
        assert_eq!(c.snippet, "Structure proposed.");
        assert_eq!(c.date.as_deref(), Some("1953-04-25"));
        assert_eq!(c.source_type, SourceType::Academic);
        assert_eq!(c.origin, "crossref");
        assert_eq!(c.extra["doi"], "10.1038/171737a0");
    }

    #[test]
    fn item_without_abstract_uses_container_and_type() {
        let item = json!({
            "title": ["Some Paper"],
            "DOI": "10.1/x",
            "container-title": ["J. Testing"],
            "type": "journal-article"
        });
        let c = item_to_candidate(&item);
        assert_eq!(c.raw_url, "https://doi.org/10.1/x");
        assert_eq!(c.snippet, "J. Testing · journal-article");
    }
}
