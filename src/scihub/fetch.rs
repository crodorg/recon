//! Fetch a paper's full-text PDF from Sci-Hub by DOI or PMID.
//!
//! Flow: parse the identifier (a PMID is resolved to a DOI via NCBI eutils, since
//! Sci-Hub is keyed by DOI) → ask each live mirror domain for the article page →
//! pull the embedded PDF URL out of the page → download → validate the bytes are
//! actually a PDF. A miss returns `found:false` with a reason, never a fabricated
//! hit. Output is plain JSON; nothing here is registered as a citable source.

use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;

use crate::http;
use crate::scihub::mirror;

/// Smaller than this ⇒ an error/captcha stub, not a paper.
const MIN_PDF_BYTES: usize = 1024;

/// PDF-download timeout (seconds) — papers can be several MB.
const DOWNLOAD_TIMEOUT_SECS: u64 = 60;

/// Result of a fetch attempt. `found` is the contract; the rest is detail.
#[derive(Debug, Serialize)]
pub struct FetchResult {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pmid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain_used: Option<String>,
    pub domains_tried: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl FetchResult {
    fn miss(doi: Option<String>, pmid: Option<String>, tried: Vec<String>, note: &str) -> Self {
        FetchResult {
            found: false,
            doi,
            pmid,
            pdf_path: None,
            domain_used: None,
            domains_tried: tried,
            note: Some(note.to_string()),
        }
    }
}

enum PaperId {
    Doi(String),
    Pmid(String),
}

/// Classify user input into a DOI or a PMID.
/// - `pmid:NNN` or a bare all-digit token ⇒ PMID (PubMed IDs are numeric).
/// - `doi:...`, a `doi.org/...` URL, or a bare `10.x/...` ⇒ DOI.
fn parse_id(input: &str) -> Option<PaperId> {
    let t = input.trim();
    let lower = t.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("pmid:") {
        let n = rest.trim();
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            return Some(PaperId::Pmid(n.to_string()));
        }
    }
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        return Some(PaperId::Pmid(t.to_string()));
    }
    normalize_doi(t).map(PaperId::Doi)
}

/// Pull a bare DOI (`10.xxxx/...`) out of a raw string: `doi:` prefix, a
/// `doi.org/` URL, or an already-bare DOI. Lowercased; fragment/query trimmed.
fn normalize_doi(raw: &str) -> Option<String> {
    let t = raw.trim();
    let lower = t.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("doi:") {
        return clean_doi(rest);
    }
    if let Some(idx) = lower.find("doi.org/") {
        return clean_doi(&t[idx + "doi.org/".len()..]);
    }
    if lower.starts_with("10.") {
        return clean_doi(t);
    }
    None
}

fn clean_doi(s: &str) -> Option<String> {
    let d = s
        .trim()
        .trim_start_matches('/')
        .split(['#', '?', ' '])
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    (d.starts_with("10.") && d.contains('/')).then_some(d)
}

/// Resolve a PMID to its DOI via NCBI eutils (free, no key). `None` if the record
/// carries no DOI.
async fn pmid_to_doi(client: &reqwest::Client, pmid: &str) -> Result<Option<String>> {
    let url = format!(
        "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi?db=pubmed&id={pmid}&retmode=json"
    );
    let v = http::get_json(client, &url).await?;
    let ids = v
        .get("result")
        .and_then(|r| r.get(pmid))
        .and_then(|p| p.get("articleids"))
        .and_then(|a| a.as_array());
    if let Some(arr) = ids {
        for id in arr {
            if id.get("idtype").and_then(|x| x.as_str()) == Some("doi") {
                if let Some(val) = id.get("value").and_then(|x| x.as_str()) {
                    if let Some(doi) = normalize_doi(val) {
                        return Ok(Some(doi));
                    }
                }
            }
        }
    }
    Ok(None)
}

/// Extract the embedded PDF URL from a Sci-Hub article page. Handles the
/// `<embed src=...>` / `<iframe src=...>` forms and the `location.href='...'`
/// save-button form, plus scheme-relative (`//host/..`) and host-relative
/// (`/downloads/..`) URLs. Returns an absolute https URL, or `None` when the page
/// carries no PDF (a miss / "article not found"). Shared with the mirror probe.
pub(crate) fn extract_pdf_url(html: &str, host: &str) -> Option<String> {
    // src="..." / src='...' (embed, iframe) — the common case.
    let src_re = Regex::new(r#"(?i)src\s*=\s*["']([^"']+)["']"#).unwrap();
    // location.href='...' — the save-button fallback some skins use.
    let href_re = Regex::new(r#"(?i)location\.href\s*=\s*['"]([^'"]+)['"]"#).unwrap();

    let candidates = src_re
        .captures_iter(html)
        .chain(href_re.captures_iter(html))
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()));

    for raw in candidates {
        if raw.to_ascii_lowercase().contains(".pdf") {
            return Some(absolutize(&raw, host));
        }
    }
    None
}

/// Turn a possibly-relative src into an absolute https URL.
fn absolutize(raw: &str, host: &str) -> String {
    let r = raw.trim();
    if r.starts_with("http://") || r.starts_with("https://") {
        r.to_string()
    } else if let Some(rest) = r.strip_prefix("//") {
        format!("https://{rest}")
    } else if let Some(rest) = r.strip_prefix('/') {
        format!("https://{host}/{rest}")
    } else {
        format!("https://{host}/{r}")
    }
}

/// A real PDF starts with `%PDF` and isn't a tiny stub.
fn is_pdf(bytes: &[u8]) -> bool {
    bytes.len() >= MIN_PDF_BYTES && bytes.starts_with(b"%PDF")
}

/// Filesystem-safe name for a DOI.
fn doi_filename(doi: &str) -> String {
    let safe: String = doi
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("sci-hub-{safe}.pdf")
}

/// Download the PDF, sending the article page as Referer (some mirrors require
/// it). Returns the validated bytes, or `None` on any failure / non-PDF body.
async fn download_pdf(client: &reqwest::Client, pdf_url: &str, referer: &str) -> Option<Vec<u8>> {
    let resp = client
        .get(pdf_url)
        .header(reqwest::header::REFERER, referer)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let bytes = resp.bytes().await.ok()?.to_vec();
    is_pdf(&bytes).then_some(bytes)
}

/// Fetch a paper by DOI or PMID into `out_dir`. Never panics on a miss — returns
/// a `FetchResult` either way.
pub async fn fetch_paper(input: &str, out_dir: &str, force_refresh: bool) -> Result<FetchResult> {
    let client = http::client();

    // 1) identifier → DOI
    let (doi, pmid) = match parse_id(input) {
        Some(PaperId::Doi(d)) => (Some(d), None),
        Some(PaperId::Pmid(p)) => (pmid_to_doi(&client, &p).await.unwrap_or(None), Some(p)),
        None => {
            return Ok(FetchResult::miss(
                None,
                None,
                vec![],
                &format!("could not parse '{input}' as a DOI or PMID"),
            ));
        }
    };
    let doi = match doi {
        Some(d) => d,
        None => {
            return Ok(FetchResult::miss(
                None,
                pmid,
                vec![],
                "PubMed record carries no DOI; Sci-Hub is keyed by DOI",
            ));
        }
    };

    // 2) live mirror domains (self-refreshing)
    let live = mirror::live_domains(force_refresh).await?;
    let tried: Vec<String> = live.iter().map(|d| d.host.clone()).collect();
    if live.is_empty() {
        return Ok(FetchResult::miss(
            Some(doi),
            pmid,
            tried,
            "no live Sci-Hub domain found — add one to ~/.config/research/scihub.conf and re-run with --refresh",
        ));
    }

    // 3) try each live domain in latency order
    let dl = http::client_timeout(DOWNLOAD_TIMEOUT_SECS);
    for d in &live {
        let page = format!("https://{}/{}", d.host, doi);
        let html = match http::get_text(&client, &page).await {
            Ok(h) => h,
            Err(_) => continue,
        };
        let pdf_url = match extract_pdf_url(&html, &d.host) {
            Some(u) => u,
            None => continue,
        };
        let bytes = match download_pdf(&dl, &pdf_url, &page).await {
            Some(b) => b,
            None => continue,
        };
        std::fs::create_dir_all(out_dir).with_context(|| format!("create out dir {out_dir}"))?;
        let path = Path::new(out_dir).join(doi_filename(&doi));
        std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
        return Ok(FetchResult {
            found: true,
            doi: Some(doi),
            pmid,
            pdf_path: Some(path.to_string_lossy().to_string()),
            domain_used: Some(d.host.clone()),
            domains_tried: tried,
            note: None,
        });
    }

    // 4) honest miss — name the most likely reason
    Ok(FetchResult::miss(
        Some(doi),
        pmid,
        tried,
        "not in Sci-Hub. Its corpus has been frozen since 2021 — papers published 2022+ are not indexed.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_scheme_relative_embed() {
        let html = r#"<embed type="application/pdf" src="//dacemirror.sci-hub.se/journal/x/paper.pdf#view=FitH" id="pdf">"#;
        assert_eq!(
            extract_pdf_url(html, "sci-hub.se").as_deref(),
            Some("https://dacemirror.sci-hub.se/journal/x/paper.pdf#view=FitH")
        );
    }

    #[test]
    fn extracts_host_relative_iframe() {
        let html = r#"<iframe src="/downloads/2021/aa/paper.pdf"></iframe>"#;
        assert_eq!(
            extract_pdf_url(html, "sci-hub.st").as_deref(),
            Some("https://sci-hub.st/downloads/2021/aa/paper.pdf")
        );
    }

    #[test]
    fn extracts_location_href_savebutton() {
        let html = r#"<button onclick="location.href='https://twin.sci-hub.ru/a/b.pdf?download=true'">save</button>"#;
        assert_eq!(
            extract_pdf_url(html, "sci-hub.ru").as_deref(),
            Some("https://twin.sci-hub.ru/a/b.pdf?download=true")
        );
    }

    #[test]
    fn no_pdf_on_article_not_found_page() {
        let html =
            r#"<html><body><p>article not found</p><img src="/misc/logo.png"></body></html>"#;
        assert_eq!(extract_pdf_url(html, "sci-hub.se"), None);
    }

    #[test]
    fn parses_doi_forms() {
        assert!(
            matches!(parse_id("10.1038/171737a0"), Some(PaperId::Doi(d)) if d == "10.1038/171737a0")
        );
        assert!(matches!(parse_id("doi:10.1000/Xyz"), Some(PaperId::Doi(d)) if d == "10.1000/xyz"));
        assert!(
            matches!(parse_id("https://doi.org/10.1000/abc#sec"), Some(PaperId::Doi(d)) if d == "10.1000/abc")
        );
    }

    #[test]
    fn parses_pmid_forms() {
        assert!(matches!(parse_id("pmid:12345678"), Some(PaperId::Pmid(p)) if p == "12345678"));
        assert!(matches!(parse_id("  12345678 "), Some(PaperId::Pmid(p)) if p == "12345678"));
    }

    #[test]
    fn rejects_garbage_and_non_doi() {
        assert!(parse_id("not a paper").is_none());
        assert!(normalize_doi("11.123/not-a-doi").is_none());
    }

    #[test]
    fn validates_pdf_magic_and_size() {
        let mut good = b"%PDF-1.7\n".to_vec();
        good.extend(std::iter::repeat_n(b'x', MIN_PDF_BYTES));
        assert!(is_pdf(&good));
        assert!(!is_pdf(b"%PDF-1.7")); // too short
        assert!(!is_pdf(&[b'P'; 4096])); // wrong magic
    }

    #[test]
    fn doi_filename_is_filesystem_safe() {
        assert_eq!(
            doi_filename("10.1038/171737a0"),
            "sci-hub-10.1038_171737a0.pdf"
        );
    }
}
