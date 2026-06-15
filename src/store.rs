//! On-disk run substrate: the append-only JSONL store + run manifest.
//!
//! Layout: `<root>/<UTC-timestamp>-<slug>/{run_manifest.json, sources.jsonl,
//! evidence.jsonl, claims.jsonl}`. IDs/timestamps are filled here so callers can
//! pass partially-populated records.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::model::{canonical_locator, now_iso, source_id, Claim, Evidence, RunManifest, Source};

const SOURCES_FILE: &str = "sources.jsonl";
const EVIDENCE_FILE: &str = "evidence.jsonl";
const CLAIMS_FILE: &str = "claims.jsonl";
const MANIFEST_FILE: &str = "run_manifest.json";

/// Default runs root: `$HOME/.local/share/recon/runs`.
pub fn default_runs_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".local").join("share").join("recon").join("runs")
}

/// Slug: sanitized, lowercased, first ~6 words of the query joined by `-`.
fn slugify(query: &str) -> String {
    let words: Vec<String> = query
        .split_whitespace()
        .take(6)
        .map(|w| {
            w.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
        })
        .collect();
    let joined = words.join("-");
    // collapse runs of '-' and trim
    let mut out = String::with_capacity(joined.len());
    let mut prev_dash = false;
    for c in joined.chars() {
        if c == '-' {
            if !prev_dash {
                out.push('-');
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "query".to_string()
    } else {
        trimmed
    }
}

/// Create a new run directory `<root>/<UTC-timestamp>-<slug>` (mkdir -p).
pub fn new_run_dir(query: &str, out_dir: Option<&str>) -> Result<PathBuf> {
    let root = match out_dir {
        Some(d) => PathBuf::from(d),
        None => default_runs_root(),
    };
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let dir = root.join(format!("{}-{}", timestamp, slugify(query)));
    fs::create_dir_all(&dir).with_context(|| format!("create run dir {}", dir.display()))?;
    Ok(dir)
}

/// Append one record as a single JSON line.
pub fn append_jsonl<T: Serialize>(path: impl AsRef<Path>, record: &T) -> Result<()> {
    let path = path.as_ref();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {} for append", path.display()))?;
    let line = serde_json::to_string(record).context("serialize jsonl record")?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// Read every record from a JSONL file. Missing file => empty Vec.
pub fn read_jsonl<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<Vec<T>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("read line {} of {}", i + 1, path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: T = serde_json::from_str(&line)
            .with_context(|| format!("parse line {} of {}", i + 1, path.display()))?;
        out.push(record);
    }
    Ok(out)
}

/// Initialize a run dir: write the manifest (pretty) + create empty JSONL files.
pub fn init_run(dir: impl AsRef<Path>, manifest: &RunManifest) -> Result<()> {
    let dir = dir.as_ref();
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;

    let manifest_path = dir.join(MANIFEST_FILE);
    let json = serde_json::to_string_pretty(manifest).context("serialize manifest")?;
    fs::write(&manifest_path, json)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    for f in [SOURCES_FILE, EVIDENCE_FILE, CLAIMS_FILE] {
        let p = dir.join(f);
        if !p.exists() {
            File::create(&p).with_context(|| format!("create {}", p.display()))?;
        }
    }
    Ok(())
}

/// Register a source. Fills canonical_locator / source_id / registered_at when
/// empty. Dedups on source_id (skips append if already present). Returns the id.
pub fn register_source(dir: impl AsRef<Path>, mut source: Source) -> Result<String> {
    if source.canonical_locator.is_empty() {
        source.canonical_locator = canonical_locator(&source.raw_url);
    }
    if source.source_id.is_empty() {
        source.source_id = source_id(&source.canonical_locator);
    }
    if source.registered_at.is_empty() {
        source.registered_at = now_iso();
    }

    let id = source.source_id.clone();
    let path = dir.as_ref().join(SOURCES_FILE);

    // The deep engine fires concurrent `recon retrieve` processes into one run
    // dir (e.g. Grok-X at t=0 alongside a Perplexity round). With multi-KB snippet
    // lines, an unguarded read-dedup-append would race: two writers could both miss
    // a source and double-append, or interleave a >PIPE_BUF line and corrupt the
    // JSONL. Hold an exclusive advisory lock (std, stable since 1.89) across the
    // whole critical section; other writers block until we're done.
    let lock = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {} for lock", path.display()))?;
    lock.lock()
        .with_context(|| format!("lock {}", path.display()))?;

    let result = (|| {
        let existing: Vec<Source> = read_jsonl(&path)?;
        if existing.iter().any(|s| s.source_id == id) {
            return Ok(false); // already present, nothing appended
        }
        append_jsonl(&path, &source)?;
        Ok::<bool, anyhow::Error>(true)
    })();

    let _ = lock.unlock(); // best-effort; the lock also releases on fd close
    result?;
    Ok(id)
}

/// Add an evidence record. Fills evidence_id / captured_at when empty.
pub fn add_evidence(dir: impl AsRef<Path>, mut evidence: Evidence) -> Result<String> {
    if evidence.captured_at.is_empty() {
        evidence.captured_at = now_iso();
    }
    if evidence.evidence_id.is_empty() {
        let locator = evidence.locator.clone().unwrap_or_default();
        evidence.evidence_id =
            crate::model::evidence_id(&evidence.source_id, &evidence.quote, &locator);
    }
    let id = evidence.evidence_id.clone();
    let path = dir.as_ref().join(EVIDENCE_FILE);
    append_jsonl(&path, &evidence)?;
    Ok(id)
}

/// Add a claim. Fills claim_id / extracted_at when empty; defaults
/// support_status to "unverified".
pub fn add_claim(dir: impl AsRef<Path>, mut claim: Claim) -> Result<String> {
    if claim.extracted_at.is_empty() {
        claim.extracted_at = now_iso();
    }
    if claim.support_status.is_empty() {
        claim.support_status = "unverified".to_string();
    }
    if claim.claim_id.is_empty() {
        claim.claim_id = crate::model::claim_id(&claim.section_id, &claim.text);
    }
    let id = claim.claim_id.clone();
    let path = dir.as_ref().join(CLAIMS_FILE);
    append_jsonl(&path, &claim)?;
    Ok(id)
}

/// Overwrite claims.jsonl with the given claims.
pub fn write_claims(dir: impl AsRef<Path>, claims: &[Claim]) -> Result<()> {
    let path = dir.as_ref().join(CLAIMS_FILE);
    let mut file = File::create(&path).with_context(|| format!("create {}", path.display()))?;
    for claim in claims {
        let line = serde_json::to_string(claim).context("serialize claim")?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MetadataStatus, Source, SourceType};
    use std::sync::Arc;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("recon-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mk_source(url: &str) -> Source {
        Source {
            source_id: String::new(),
            canonical_locator: String::new(),
            raw_url: url.to_string(),
            title: format!("title for {url}"),
            authors: None,
            year: None,
            date: None,
            source_type: SourceType::Web,
            // A >PIPE_BUF line so the test actually stresses append atomicity:
            // a torn interleave would make read_jsonl fail to parse.
            snippet: Some("x".repeat(5000)),
            origin: "test".to_string(),
            metadata_status: MetadataStatus::Unverified,
            registered_at: String::new(),
            extra: serde_json::Value::Null,
            credibility: None,
        }
    }

    // Many writers, the same 5 URLs, large lines. Without the lock this races
    // into duplicate rows and/or corrupted (un-parseable) JSONL; with it, the
    // read-dedup-append section is serialized → exactly 5 clean unique rows.
    #[test]
    fn concurrent_register_dedups_and_does_not_corrupt() {
        let dir = Arc::new(scratch_dir("concurrent"));
        let urls: Vec<String> = (0..5).map(|i| format!("https://example.com/{i}")).collect();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let dir = Arc::clone(&dir);
            let urls = urls.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..10 {
                    for u in &urls {
                        register_source(dir.as_ref(), mk_source(u)).unwrap();
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Must parse cleanly (no torn/interleaved lines) ...
        let sources: Vec<Source> = read_jsonl(dir.join(SOURCES_FILE)).unwrap();
        let mut ids: Vec<String> = sources.iter().map(|s| s.source_id.clone()).collect();
        ids.sort();
        ids.dedup();
        // ... and dedup must have held despite 400 racing attempts on 5 URLs.
        assert_eq!(sources.len(), 5, "expected 5 rows, got {}", sources.len());
        assert_eq!(ids.len(), 5, "expected 5 unique ids, got {}", ids.len());

        let _ = fs::remove_dir_all(dir.as_ref());
    }
}
