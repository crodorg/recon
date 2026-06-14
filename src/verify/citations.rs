//! Citation verification — port of `verify_citations.py` (CiteGuard heuristics +
//! DOI/URL resolution).
//!
//! For a single [`CitationEntry`] we (1) run hallucination-pattern detection on
//! the title, (2) resolve the DOI via doi.org content negotiation and compare
//! title/year, (3) fall back to a HEAD check on the URL, and (4) mark entries
//! with no DOI and no URL as suspicious. The collected `status`, `issues`, and
//! `methods` are returned as a [`CitationVerdict`].

use std::time::Duration;

use regex::Regex;

use crate::model::{CitationEntry, CitationVerdict};

/// Timeout for the per-citation DOI / URL network checks (the Python script uses
/// 10s; the shared `http::client()` default is 20s, so we override per request).
const NET_TIMEOUT: Duration = Duration::from_secs(10);

/// HEAD requests pretend to be a browser, matching the Python verifier's UA so
/// servers that reject the default crawler UA still answer.
const URL_CHECK_UA: &str = "Mozilla/5.0 (Research Citation Verifier)";

pub async fn verify(
    client: &reqwest::Client,
    entry: &crate::model::CitationEntry,
) -> crate::model::CitationVerdict {
    let mut issues: Vec<String> = Vec::new();
    let mut methods: Vec<String> = Vec::new();

    // STEP 1: hallucination-pattern detection (CiteGuard).
    let hallucination_issues = detect_hallucination_patterns(entry);
    let mut status = if hallucination_issues.is_empty() {
        "unknown".to_string()
    } else {
        issues.extend(hallucination_issues);
        "suspicious".to_string()
    };

    // STEP 2: has DOI? Resolve via doi.org content negotiation.
    if let Some(doi) = non_empty(&entry.doi) {
        match resolve_doi(client, doi).await {
            DoiOutcome::Resolved { title, year } => {
                status = "verified".to_string();

                // Title similarity check (only when both titles are present).
                if let (Some(entry_title), Some(meta_title)) =
                    (non_empty(&entry.title), title.as_deref().filter(|t| !t.is_empty()))
                {
                    let similarity = title_similarity(entry_title, meta_title);
                    if similarity < 0.5 {
                        issues.push(format!(
                            "Title mismatch (similarity: {:.1}%)",
                            similarity * 100.0
                        ));
                        status = "suspicious".to_string();
                    }
                }

                // Year match check (only when both years parse as integers).
                if let (Some(entry_year), Some(meta_year)) =
                    (parse_year(&entry.year), year)
                {
                    if entry_year != meta_year {
                        issues.push(format!(
                            "Year mismatch: report says {entry_year}, DOI says {meta_year}"
                        ));
                        status = "suspicious".to_string();
                    }
                }
            }
            DoiOutcome::Failed(err) => {
                status = "unverified".to_string();
                issues.push(format!("DOI resolution failed: {err}"));
            }
        }
    }

    // STEP 3: URL accessibility (when there is no DOI or the DOI did not verify).
    if let Some(url) = non_empty(&entry.url) {
        if status != "verified" {
            match check_url(client, url).await {
                Ok(()) => {
                    methods.push("URL".to_string());
                    if matches!(status.as_str(), "unknown" | "no_doi" | "unverified") {
                        status = "url_verified".to_string();
                    }
                }
                Err(msg) => {
                    issues.push(format!("URL check failed: {msg}"));
                }
            }
        }
    }

    // STEP 4: no DOI and no URL — nothing to verify against.
    if non_empty(&entry.doi).is_none() && non_empty(&entry.url).is_none() {
        if !issues.iter().any(|i| i.contains("No DOI provided")) {
            issues.push("No DOI or URL - cannot verify".to_string());
        }
        status = "suspicious".to_string();
    }

    CitationVerdict {
        num: entry.num.clone(),
        status,
        issues,
        methods,
    }
}

// ---------------------------------------------------------------------------
// DOI resolution
// ---------------------------------------------------------------------------

enum DoiOutcome {
    /// Resolved metadata: parsed title (CSL `title`) and year (`issued`).
    Resolved {
        title: Option<String>,
        year: Option<i64>,
    },
    /// Resolution failed (404, other HTTP status, transport error).
    Failed(String),
}

/// GET `https://doi.org/<doi>` with CSL+JSON content negotiation (10s). On 200,
/// parse `title` (string or array) and `issued.date-parts[0][0]` (year).
async fn resolve_doi(client: &reqwest::Client, doi: &str) -> DoiOutcome {
    let url = format!("https://doi.org/{}", encode_doi_path(doi));

    let resp = match client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/vnd.citationstyles.csl+json")
        .timeout(NET_TIMEOUT)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return DoiOutcome::Failed(short_err(&e.to_string())),
    };

    let status = resp.status();
    if status.as_u16() == 404 {
        return DoiOutcome::Failed("DOI not found (404)".to_string());
    }
    if !status.is_success() {
        return DoiOutcome::Failed(format!("HTTP {}", status.as_u16()));
    }

    let value: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return DoiOutcome::Failed(short_err(&e.to_string())),
    };

    let title = csl_title(&value);
    let year = value
        .get("issued")
        .and_then(|i| i.get("date-parts"))
        .and_then(|dp| dp.get(0))
        .and_then(|first| first.get(0))
        .and_then(csl_int);

    DoiOutcome::Resolved { title, year }
}

/// CSL `title` may be a JSON string or an array of strings; normalize to a
/// single string (joining array parts with a space, mirroring typical usage).
fn csl_title(value: &serde_json::Value) -> Option<String> {
    match value.get("title") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(arr)) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(" "))
            }
        }
        _ => None,
    }
}

/// A CSL date-part may be a JSON number or a numeric string; coerce to i64.
fn csl_int(value: &serde_json::Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    value.as_str().and_then(|s| s.trim().parse::<i64>().ok())
}

/// Percent-encode the DOI for use as a single path segment (mirrors Python's
/// `urllib.parse.quote(doi)` with default `safe='/'`).
fn encode_doi_path(doi: &str) -> String {
    let mut out = String::with_capacity(doi.len());
    for b in doi.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/');
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// URL accessibility
// ---------------------------------------------------------------------------

/// HEAD the URL (10s, browser-ish UA). Ok(()) on 200; Err(message) otherwise.
async fn check_url(client: &reqwest::Client, url: &str) -> Result<(), String> {
    let resp = client
        .head(url)
        .header(reqwest::header::USER_AGENT, URL_CHECK_UA)
        .timeout(NET_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("URL error: {}", short_err(&e.to_string())))?;

    if resp.status().as_u16() == 200 {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status().as_u16()))
    }
}

// ---------------------------------------------------------------------------
// Hallucination-pattern detection
// ---------------------------------------------------------------------------

fn detect_hallucination_patterns(entry: &CitationEntry) -> Vec<String> {
    let mut issues = Vec::new();

    let title = match non_empty(&entry.title) {
        Some(t) => t,
        None => return issues,
    };
    let title_lower = title.to_lowercase();

    // Suspicious templated/generic title patterns (case-insensitive, anchored at
    // start to match Python's `re.match`).
    for (pattern, description) in suspicious_patterns() {
        if pattern.is_match(title) {
            issues.push(format!("Suspicious title pattern: {description}"));
        }
    }

    // Overly generic short title.
    let generic_words = ["overview", "introduction", "guide", "handbook", "manual"];
    let word_count = title.split_whitespace().count();
    if word_count < 5 && generic_words.iter().any(|w| title_lower.contains(w)) {
        issues.push("Very generic short title".to_string());
    }

    // Placeholder text.
    let placeholders = ["tbd", "todo", "placeholder", "example"];
    if placeholders.iter().any(|p| title_lower.contains(p)) {
        issues.push("Placeholder text in title".to_string());
    }

    // Year-based heuristics (only when the year parses as an integer).
    if let Some(year) = parse_year(&entry.year) {
        let current_year = chrono::Local::now().format("%Y").to_string();
        let current_year: i64 = current_year.parse().unwrap_or(0);

        // Recent year with no verification method.
        if year >= current_year - 1
            && non_empty(&entry.doi).is_none()
            && non_empty(&entry.url).is_none()
        {
            issues.push(format!("Recent year ({year}) with no verification method"));
        }
        // Future year.
        if year > current_year {
            issues.push(format!(
                "Future year: {year} (current: {current_year})"
            ));
        }
        // Anachronistic modern-AI terms in a pre-2000 title.
        if year < 2000 {
            let modern = ["ai", "llm", "gpt", "transformer"];
            if modern.iter().any(|w| title_lower.contains(w)) {
                issues.push(format!(
                    "Anachronistic: pre-2000 ({year}) citation mentioning modern AI terms"
                ));
            }
        }
    }

    issues
}

/// The CiteGuard suspicious-title regexes. Compiled per call (one entry at a
/// time, so this is not hot); kept local to this file per the no-shared-state
/// rule. Patterns are case-insensitive and anchored to match Python `re.match`.
fn suspicious_patterns() -> Vec<(Regex, &'static str)> {
    let specs: [(&str, &str); 3] = [
        (
            r"(?i)^(A |An |The )?(Study|Analysis|Review|Survey|Investigation) (of|on|into)",
            "Generic academic title pattern",
        ),
        (
            r"(?i)^(Recent|Current|Modern|Contemporary) (Advances|Developments|Trends) in",
            "Generic 'advances' title pattern",
        ),
        (
            r"(?i)^[A-Z][a-z]+ [A-Z][a-z]+: A (Comprehensive|Complete|Systematic) (Review|Analysis|Guide)$",
            "Too perfect, templated structure",
        ),
    ];
    specs
        .iter()
        .filter_map(|(p, d)| Regex::new(p).ok().map(|re| (re, *d)))
        .collect()
}

// ---------------------------------------------------------------------------
// Title similarity
// ---------------------------------------------------------------------------

/// Word-overlap similarity (Jaccard) of two titles, 0.0..=1.0. Normalizes by
/// lowercasing and replacing non-word/non-space chars with spaces, then
/// comparing the sets of resulting tokens.
fn title_similarity(title1: &str, title2: &str) -> f64 {
    let words1 = normalize_words(title1);
    let words2 = normalize_words(title2);

    if words1.is_empty() || words2.is_empty() {
        return 0.0;
    }

    let overlap = words1.intersection(&words2).count();
    let total = words1.union(&words2).count();
    if total == 0 {
        0.0
    } else {
        overlap as f64 / total as f64
    }
}

/// Lowercase, map every char that is not alphanumeric/underscore/whitespace to a
/// space (matching Python `re.sub(r'[^\w\s]', ' ', s)` for ASCII text), then
/// split on whitespace into a set of tokens.
fn normalize_words(s: &str) -> std::collections::HashSet<String> {
    let cleaned: String = s
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    cleaned.split_whitespace().map(|w| w.to_string()).collect()
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Treat an `Option<String>` as present only when it is `Some` and non-empty,
/// returning the inner `&str`. Mirrors the Python truthiness checks on entry
/// fields (empty string is falsy there).
fn non_empty(opt: &Option<String>) -> Option<&str> {
    opt.as_deref().filter(|s| !s.is_empty())
}

/// Parse a year field (`Option<String>`) into an integer, ignoring surrounding
/// whitespace. None when absent/empty/non-numeric — those callers simply skip
/// the year-dependent checks, as the Python does on `int()` failure paths.
fn parse_year(opt: &Option<String>) -> Option<i64> {
    non_empty(opt).and_then(|s| s.trim().parse::<i64>().ok())
}

/// Truncate a transport error message to the first 50 chars, matching the
/// Python `str(e)[:50]` behavior for connection errors.
fn short_err(msg: &str) -> String {
    msg.chars().take(50).collect()
}
