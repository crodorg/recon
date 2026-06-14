//! Frozen data model + deterministic ID/canonicalization helpers.
//!
//! Every type here is the binary↔skill contract. IDs are content-hashed and
//! stable across runs so they survive context compaction; display numbers
//! (`[1] [2]`) are derived at render time, never stored.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Web,
    Academic,
    Documentation,
    Code,
    News,
    Government,
    Book,
    Social,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    DirectQuote,
    Paraphrase,
    DataPoint,
    FigureReference,
    Methodology,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimType {
    Factual,
    Synthesis,
    Recommendation,
    Speculation,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MetadataStatus {
    Unverified,
    DoiVerified,
    UrlVerified,
    TitleMatched,
}

// ---------------------------------------------------------------------------
// Core records (the .jsonl substrate)
// ---------------------------------------------------------------------------

/// Defaults for fields the `store` fills in when left empty, so callers of
/// `register-source` / `add-evidence` / `add-claim` can pass minimal JSON (the
/// content-hashed IDs and timestamps are derived on write, not supplied).
fn default_metadata_status() -> MetadataStatus {
    MetadataStatus::Unverified
}
fn default_evidence_type() -> EvidenceType {
    EvidenceType::DirectQuote
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Source {
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub canonical_locator: String,
    pub raw_url: String,
    pub title: String,
    pub authors: Option<Vec<String>>,
    pub year: Option<String>,
    /// Publication / last-updated date as reported by the connector. Kept for
    /// currency + as-of reasoning in the verification layer.
    #[serde(default)]
    pub date: Option<String>,
    pub source_type: SourceType,
    /// Retrieval-time excerpt from the connector (Perplexity's `snippet`, an HN
    /// comment, a repo description, ...). Persisted so the triage pass and shallow
    /// secondary evidence can read it from disk without re-fetching. May be long
    /// for Perplexity (page content scaled by `max_tokens_per_page`).
    #[serde(default)]
    pub snippet: Option<String>,
    /// Which connector surfaced this source (e.g. "perplexity", "hackernews",
    /// "github", "grok-x"). Provenance / modality label.
    #[serde(default)]
    pub origin: String,
    #[serde(default = "default_metadata_status")]
    pub metadata_status: MetadataStatus,
    #[serde(default)]
    pub registered_at: String,
    /// Per-connector signal (HN points, GitHub stars, Perplexity rank, ...).
    #[serde(default)]
    pub extra: serde_json::Value,
    pub credibility: Option<Credibility>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Evidence {
    #[serde(default)]
    pub evidence_id: String,
    pub source_id: String,
    pub retrieval_query: Option<String>,
    pub locator: Option<String>,
    pub quote: String,
    #[serde(default = "default_evidence_type")]
    pub evidence_type: EvidenceType,
    /// How the quote was obtained: `primary_fetch` (the reader fetched the full
    /// page) vs `excerpt` (pulled from the connector's snippet without a fetch).
    /// Lets synthesis know what rests on an excerpt. None = unspecified (treat as
    /// primary_fetch for back-compat with pre-Phase-3 rows).
    #[serde(default)]
    pub provenance: Option<String>,
    #[serde(default)]
    pub captured_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Claim {
    #[serde(default)]
    pub claim_id: String,
    pub section_id: String,
    pub text: String,
    pub claim_type: ClaimType,
    #[serde(default)]
    pub cited_source_ids: Vec<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub support_status: String,
    #[serde(default)]
    pub extracted_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Assumption {
    pub assumption_id: String,
    pub text: String,
    pub materiality: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunManifest {
    pub version: String,
    pub query: String,
    pub mode: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub research_as_of: String,
    pub report_dir: String,
    pub assumptions: Vec<Assumption>,
}

// ---------------------------------------------------------------------------
// Retrieval + verification value types
// ---------------------------------------------------------------------------

/// The normalized search hit EVERY connector returns.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Candidate {
    pub raw_url: String,
    pub title: String,
    pub snippet: String,
    pub date: Option<String>,
    pub source_type: SourceType,
    pub origin: String,
    pub extra: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Credibility {
    pub overall: f64,
    pub domain: f64,
    pub recency: f64,
    pub expertise: f64,
    pub neutrality: f64,
    pub recommendation: String,
    pub factors: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CitationEntry {
    pub num: Option<String>,
    pub title: Option<String>,
    pub year: Option<String>,
    pub doi: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CitationVerdict {
    pub num: Option<String>,
    pub status: String,
    pub issues: Vec<String>,
    pub methods: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SupportResult {
    pub status: String,
    pub score: f64,
    pub notes: String,
}

// ---------------------------------------------------------------------------
// Canonicalization + ID helpers
// ---------------------------------------------------------------------------

/// Tracking query parameters dropped during canonicalization.
const TRACKING_PARAMS: &[&str] = &[
    "fbclid", "gclid", "msclkid", "dclid", "ref", "ref_src", "ref_url", "referrer", "source",
    "mc_cid", "mc_eid", "igshid", "spm", "_hsenc", "_hsmi",
];

fn is_tracking_param(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.starts_with("utm_") || TRACKING_PARAMS.contains(&k.as_str())
}

/// Extract a bare DOI from a string that either starts with `doi:` or contains
/// a `doi.org/10.` segment. Returns the DOI proper (starting at `10.`).
fn extract_doi(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("doi:") {
        let start = trimmed.len() - rest.len();
        return Some(trimmed[start..].trim().trim_start_matches('/').to_string());
    }
    if let Some(idx) = lower.find("doi.org/10.") {
        // position of "10." within the original string
        let doi_start = idx + "doi.org/".len();
        let doi = trimmed[doi_start..]
            .trim()
            .trim_end_matches('/')
            .split(['#', '?'])
            .next()
            .unwrap_or("")
            .to_string();
        if !doi.is_empty() {
            return Some(doi);
        }
    }
    None
}

/// Produce a canonical locator for a raw URL or DOI.
///
/// - DOI-like input -> `doi:<doi>`.
/// - else: lowercase scheme + host, strip leading `www.`, keep path, DROP the
///   fragment and tracking query params, drop a trailing slash on the path.
pub fn canonical_locator(raw_url: &str) -> String {
    if let Some(doi) = extract_doi(raw_url) {
        return format!("doi:{}", doi.to_ascii_lowercase());
    }

    match url::Url::parse(raw_url.trim()) {
        Ok(mut parsed) => {
            let scheme = parsed.scheme().to_ascii_lowercase();

            // host: lowercase + strip leading www.
            let host = parsed
                .host_str()
                .map(|h| h.to_ascii_lowercase())
                .map(|h| h.strip_prefix("www.").map(str::to_string).unwrap_or(h));

            // path: drop a trailing slash (but keep "/" as empty path).
            let path = parsed.path().to_string();
            let path = path.trim_end_matches('/').to_string();

            // surviving query params, in original order, tracking dropped.
            let kept: Vec<(String, String)> = parsed
                .query_pairs()
                .filter(|(k, _)| !is_tracking_param(k))
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();

            // Rebuild deterministically rather than mutating in place.
            let mut out = String::new();
            out.push_str(&scheme);
            out.push_str("://");
            if let Some(h) = &host {
                out.push_str(h);
            }
            // preserve a non-default port if present
            if let Some(port) = parsed.port() {
                out.push(':');
                out.push_str(&port.to_string());
            }
            out.push_str(&path);
            if !kept.is_empty() {
                out.push('?');
                let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                for (k, v) in &kept {
                    serializer.append_pair(k, v);
                }
                out.push_str(&serializer.finish());
            }
            // fragment intentionally dropped
            let _ = &mut parsed;
            out
        }
        // Not a parseable URL: fall back to a trimmed, lowercased, de-fragmented form.
        Err(_) => raw_url
            .trim()
            .split('#')
            .next()
            .unwrap_or("")
            .trim_end_matches('/')
            .to_string(),
    }
}

/// sha256 hex, first 16 chars.
pub fn sha16(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let hex = digest
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    hex[..16].to_string()
}

/// source_id = sha16(canonical_locator).
pub fn source_id(loc: &str) -> String {
    sha16(loc)
}

/// evidence_id = sha16(source_id + quote_norm + locator).
pub fn evidence_id(source_id: &str, quote_norm: &str, locator: &str) -> String {
    sha16(&format!("{}{}{}", source_id, quote_norm, locator))
}

/// claim_id = sha16(section_id + text_norm).
pub fn claim_id(section_id: &str, text_norm: &str) -> String {
    sha16(&format!("{}{}", section_id, text_norm))
}

/// Current time as an RFC3339 / ISO-8601 string (UTC).
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Extract a 4-digit year (`19xx`/`20xx`) from a date string of any common
/// format (ISO-8601, RFC2822, bare year, ...). First match wins.
pub fn year_from_date(date: Option<&str>) -> Option<String> {
    let d = date?;
    for w in d.as_bytes().windows(4) {
        if (&w[..2] == b"19" || &w[..2] == b"20") && w.iter().all(|c| c.is_ascii_digit()) {
            return Some(String::from_utf8_lossy(w).into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // The store fills content-hashed IDs + timestamps on write, so the CLI
    // add-* commands must accept minimal JSON without those fields. These guard
    // the serde defaults that make that work.
    #[test]
    fn evidence_minimal_json_fills_defaults() {
        let e: Evidence =
            serde_json::from_str(r#"{"source_id":"s","quote":"q","locator":"l"}"#).unwrap();
        assert_eq!(e.evidence_id, "");
        assert_eq!(e.evidence_type, EvidenceType::DirectQuote);
        assert_eq!(e.captured_at, "");
        assert_eq!(e.provenance, None);
    }

    #[test]
    fn evidence_accepts_provenance() {
        let e: Evidence =
            serde_json::from_str(r#"{"source_id":"s","quote":"q","provenance":"primary_fetch"}"#)
                .unwrap();
        assert_eq!(e.provenance.as_deref(), Some("primary_fetch"));
    }

    #[test]
    fn claim_minimal_json_fills_defaults() {
        let c: Claim =
            serde_json::from_str(r#"{"section_id":"intro","text":"t","claim_type":"factual"}"#)
                .unwrap();
        assert_eq!(c.claim_id, "");
        assert!(c.cited_source_ids.is_empty());
        assert!(c.evidence_ids.is_empty());
        assert_eq!(c.support_status, "");
        assert_eq!(c.claim_type, ClaimType::Factual);
    }

    #[test]
    fn source_minimal_json_fills_defaults() {
        let s: Source = serde_json::from_str(
            r#"{"raw_url":"https://example.com/x","title":"T","source_type":"web"}"#,
        )
        .unwrap();
        assert_eq!(s.source_id, "");
        assert_eq!(s.canonical_locator, "");
        assert_eq!(s.metadata_status, MetadataStatus::Unverified);
        assert_eq!(s.snippet, None);
    }
}
