//! research — terminal deep-research engine: retrieval + deterministic verification.

mod model;
mod http;
mod store;
mod sources;
mod scihub;
mod verify;

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::future::join_all;

use model::{
    Candidate, CitationEntry, Claim, Credibility, Evidence, MetadataStatus, RunManifest, Source,
};

#[derive(Parser)]
#[command(
    name = "research",
    version,
    about = "Terminal deep-research engine: retrieval + deterministic verification"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fan out to the selected connectors, dedup, score, and register sources.
    Retrieve {
        /// The research query.
        query: String,
        /// Research mode (e.g. quick / deep).
        #[arg(long, default_value = "quick")]
        mode: String,
        /// Comma-separated connector names. Social coverage is X via the `grok`
        /// connector; there is no Reddit connector (direct Reddit is IP-blocked
        /// without a third-party scraper, which we don't use).
        #[arg(long, default_value = "hn,github,polymarket,grok,perplexity")]
        sources: String,
        /// Max hits per connector.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Override the run output directory root.
        #[arg(long)]
        out_dir: Option<String>,
        /// Append to an existing run dir instead of creating a new one. Lets the
        /// orchestrator stage retrieval (e.g. a fast web batch, then a social
        /// batch) into one run; dedup is by source_id across batches.
        #[arg(long)]
        run_dir: Option<String>,
        /// Perplexity only: comma-separated domain allow-list (max 20, e.g.
        /// `sec.gov,ecfr.gov`). Targets primary sources. Ignored by other connectors.
        #[arg(long)]
        domains: Option<String>,
        /// Perplexity only: only results published after this date (MM/DD/YYYY).
        #[arg(long)]
        after: Option<String>,
        /// Perplexity only: only results published before this date (MM/DD/YYYY).
        #[arg(long)]
        before: Option<String>,
        /// Perplexity only: recency window — hour|day|week|month|year.
        #[arg(long)]
        recency: Option<String>,
        /// Perplexity only: page-content tokens to extract into each result's
        /// snippet/excerpt. Free (per-request pricing). 1024 ≈ ~1k-char excerpts
        /// (enough for triage); crank to ~4096 when an excerpt should yield
        /// shallow secondary evidence without a full fetch.
        #[arg(long, default_value_t = 1024)]
        max_tokens_per_page: usize,
    },
    /// Create a run dir + manifest and print its path, without retrieving. Lets
    /// the orchestrator mint the run at t=0 so every connector (Perplexity AND
    /// the slow Grok-X batch) can launch concurrently via `--run-dir`.
    InitRun {
        /// The research query (recorded in the manifest, used for the slug).
        query: String,
        /// Research mode (e.g. quick / deep).
        #[arg(long, default_value = "deep")]
        mode: String,
        /// Override the run output directory root.
        #[arg(long)]
        out_dir: Option<String>,
    },
    /// Deterministic JSON projection of a run's sources.jsonl, sorted by
    /// credibility. The triage/read fan-out reads this instead of `cat | python`,
    /// so counts and selection are disk-truthful.
    ListSources {
        #[arg(long)]
        dir: String,
    },
    /// Register a source from a JSON object.
    RegisterSource {
        #[arg(long)]
        dir: String,
        #[arg(long)]
        json: String,
    },
    /// Add an evidence record from a JSON object.
    AddEvidence {
        #[arg(long)]
        dir: String,
        #[arg(long)]
        json: String,
    },
    /// Add a claim from a JSON object.
    AddClaim {
        #[arg(long)]
        dir: String,
        #[arg(long)]
        json: String,
    },
    /// Verify citations (phantom-citation guard) against sources or a report.
    VerifyCitations {
        /// Run dir whose sources.jsonl supplies citations.
        #[arg(long)]
        dir: Option<String>,
        /// Markdown report; parse its `## Bibliography` section.
        #[arg(long)]
        report: Option<String>,
        /// Exit non-zero if any citation is suspicious/unverified.
        #[arg(long)]
        strict: bool,
    },
    /// Check claim↔evidence support and stamp claims.jsonl.
    VerifySupport {
        #[arg(long)]
        dir: String,
        /// Exit non-zero if any factual claim is unsupported.
        #[arg(long)]
        strict: bool,
    },
    /// Score a single URL's credibility.
    Score {
        #[arg(long)]
        json: String,
    },
    /// Fetch a paper's full-text PDF from Sci-Hub by DOI or PMID. A reading aid,
    /// NOT a citation source — cite the DOI/publisher, never the mirror. Honest
    /// miss (`found:false`) when the paper isn't in the corpus (frozen at 2021).
    FetchPaper {
        /// DOI (`10.1038/171737a0`), a `doi.org/...` URL, or a PMID (`pmid:123` / `123`).
        id: String,
        /// Directory to write the PDF into (default: current dir).
        #[arg(long)]
        out: Option<String>,
        /// Force a domain-mirror refresh before fetching.
        #[arg(long)]
        refresh: bool,
    },
    /// Inspect or refresh the Sci-Hub live-domain mirror (probe-curated cache).
    ScihubDomains {
        /// Re-probe all seed/config domains and rewrite the cache.
        #[arg(long)]
        refresh: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Retrieve {
            query,
            mode,
            sources,
            limit,
            out_dir,
            run_dir,
            domains,
            after,
            before,
            recency,
            max_tokens_per_page,
        } => {
            cmd_retrieve(RetrieveArgs {
                query,
                mode,
                sources,
                limit,
                out_dir,
                run_dir,
                domains,
                after,
                before,
                recency,
                max_tokens_per_page,
            })
            .await
        }
        Command::InitRun {
            query,
            mode,
            out_dir,
        } => cmd_init_run(query, mode, out_dir),
        Command::ListSources { dir } => cmd_list_sources(dir),
        Command::RegisterSource { dir, json } => cmd_register_source(dir, json),
        Command::AddEvidence { dir, json } => cmd_add_evidence(dir, json),
        Command::AddClaim { dir, json } => cmd_add_claim(dir, json),
        Command::VerifyCitations {
            dir,
            report,
            strict,
        } => cmd_verify_citations(dir, report, strict).await,
        Command::VerifySupport { dir, strict } => cmd_verify_support(dir, strict),
        Command::Score { json } => cmd_score(json),
        Command::FetchPaper { id, out, refresh } => cmd_fetch_paper(id, out, refresh).await,
        Command::ScihubDomains { refresh } => cmd_scihub_domains(refresh).await,
    }
}

// ---------------------------------------------------------------------------
// fetch-paper / scihub-domains
// ---------------------------------------------------------------------------

/// Fetch a paper PDF from Sci-Hub by DOI/PMID and print the JSON result. A miss
/// is a valid answer (`found:false`), so this still exits 0 — the JSON is the
/// contract.
async fn cmd_fetch_paper(id: String, out: Option<String>, refresh: bool) -> Result<()> {
    let out_dir = out.unwrap_or_else(|| ".".to_string());
    let result = scihub::fetch::fetch_paper(&id, &out_dir, refresh).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

/// Print the live-domain mirror; with `--refresh`, re-probe first. A read with no
/// cache yet triggers a probe so the command always returns a real picture.
async fn cmd_scihub_domains(refresh: bool) -> Result<()> {
    let cache = if refresh {
        scihub::mirror::refresh().await?
    } else {
        match scihub::mirror::read_cache() {
            Some(c) => c,
            None => scihub::mirror::refresh().await?,
        }
    };
    println!("{}", serde_json::to_string_pretty(&cache)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// retrieve
// ---------------------------------------------------------------------------

type CandFut<'a> = Pin<Box<dyn std::future::Future<Output = (String, Result<Vec<Candidate>>)> + 'a>>;

/// Resolve the run dir (append to a given one, or mint a fresh timestamped one)
/// and ensure its manifest + empty JSONL files exist. Idempotent on an existing
/// run: leaves the original query/mode/started_at untouched. Shared by `retrieve`
/// and `init-run`.
fn ensure_run(
    query: &str,
    mode: &str,
    out_dir: Option<&str>,
    run_dir: Option<&str>,
) -> Result<PathBuf> {
    let dir = match run_dir {
        Some(d) => PathBuf::from(d),
        None => store::new_run_dir(query, out_dir)?,
    };
    if !dir.join("run_manifest.json").exists() {
        let manifest = RunManifest {
            version: env!("CARGO_PKG_VERSION").to_string(),
            query: query.to_string(),
            mode: mode.to_string(),
            started_at: model::now_iso(),
            finished_at: None,
            research_as_of: chrono::Utc::now().format("%Y-%m-%d").to_string(),
            report_dir: dir.to_string_lossy().to_string(),
            assumptions: Vec::new(),
        };
        store::init_run(&dir, &manifest)?;
    }
    Ok(dir)
}

/// Parsed `retrieve` arguments (kept as a struct so the command's growing flag
/// set doesn't become an unreadable positional argument list).
struct RetrieveArgs {
    query: String,
    mode: String,
    sources: String,
    limit: usize,
    out_dir: Option<String>,
    run_dir: Option<String>,
    domains: Option<String>,
    after: Option<String>,
    before: Option<String>,
    recency: Option<String>,
    max_tokens_per_page: usize,
}

async fn cmd_retrieve(args: RetrieveArgs) -> Result<()> {
    let RetrieveArgs {
        query,
        mode,
        sources,
        limit,
        out_dir,
        run_dir,
        domains,
        after,
        before,
        recency,
        max_tokens_per_page,
    } = args;

    let run_dir = ensure_run(&query, &mode, out_dir.as_deref(), run_dir.as_deref())?;

    // Perplexity-specific search options (other connectors ignore these).
    let pplx_opts = sources::perplexity::SearchOpts {
        domains: domains
            .as_deref()
            .map(|d| {
                d.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        after,
        before,
        recency,
        max_tokens_per_page,
    };

    let client = http::client();

    let selected: Vec<String> = sources
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    // Spawn each selected connector; box the futures so the heterogeneous set
    // can live in one Vec for join_all.
    let mut futures: Vec<CandFut> = Vec::new();
    for name in &selected {
        let n = name.clone();
        let c = &client;
        let q = query.as_str();
        let fut: CandFut = match name.as_str() {
            "hn" => Box::pin(async move { (n, sources::hn::search(c, q, limit).await) }),
            "github" => Box::pin(async move { (n, sources::github::search(c, q, limit).await) }),
            "polymarket" => {
                Box::pin(async move { (n, sources::polymarket::search(c, q, limit).await) })
            }
            "grok" => Box::pin(async move { (n, sources::grok::search(c, q, limit).await) }),
            "openalex" => {
                Box::pin(async move { (n, sources::openalex::search(c, q, limit).await) })
            }
            "crossref" => {
                Box::pin(async move { (n, sources::crossref::search(c, q, limit).await) })
            }
            "reddit" => {
                Box::pin(async move { (n, sources::grok::search_reddit(c, q, limit).await) })
            }
            "perplexity" => {
                let opts = &pplx_opts;
                Box::pin(async move {
                    (n, sources::perplexity::search_with(c, q, limit, opts).await)
                })
            }
            other => {
                let other = other.to_string();
                Box::pin(async move {
                    (
                        n,
                        Err(anyhow::anyhow!("unknown source: {other}")),
                    )
                })
            }
        };
        futures.push(fut);
    }

    let results = join_all(futures).await;

    // Collect Ok candidates; a failing source logs to stderr and is skipped.
    let mut candidates: Vec<Candidate> = Vec::new();
    for (name, res) in results {
        match res {
            Ok(mut cands) => candidates.append(&mut cands),
            Err(e) => eprintln!("source {name} failed: {e:#}"),
        }
    }

    // Dedup by canonical locator, preserving first-seen order.
    let mut seen = std::collections::HashSet::new();
    let mut unique: Vec<Candidate> = Vec::new();
    for cand in candidates {
        let loc = model::canonical_locator(&cand.raw_url);
        if seen.insert(loc) {
            unique.push(cand);
        }
    }

    // Load the user trust config once (empty if absent) — overrides built-in
    // domain tiers so credibility reflects the operator's curation.
    let trust = verify::credibility::TrustConfig::load();

    // Score + register each unique candidate.
    let mut registered: Vec<Source> = Vec::new();
    for cand in unique {
        let cred: Credibility = verify::credibility::score_with(
            &cand.raw_url,
            &cand.title,
            cand.date.as_deref(),
            None,
            &trust,
        );
        let mut source = Source {
            source_id: String::new(),
            canonical_locator: String::new(),
            raw_url: cand.raw_url.clone(),
            title: cand.title.clone(),
            authors: None,
            year: model::year_from_date(cand.date.as_deref()),
            date: cand.date.clone(),
            source_type: cand.source_type.clone(),
            // Persist the retrieval-time excerpt (empty → None) so triage + shallow
            // secondary evidence can read it from disk without re-fetching.
            snippet: Some(cand.snippet.clone()).filter(|s| !s.is_empty()),
            origin: cand.origin.clone(),
            metadata_status: MetadataStatus::Unverified,
            registered_at: String::new(),
            extra: cand.extra.clone(),
            credibility: Some(cred),
        };
        let id = store::register_source(&run_dir, source.clone())?;
        // reflect the filled-in fields for output
        source.source_id = id;
        if source.canonical_locator.is_empty() {
            source.canonical_locator = model::canonical_locator(&source.raw_url);
        }
        if source.registered_at.is_empty() {
            source.registered_at = model::now_iso();
        }
        registered.push(source);
    }

    let out = serde_json::json!({
        "run_dir": run_dir.to_string_lossy(),
        "count": registered.len(),
        "sources": registered,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// init-run / list-sources
// ---------------------------------------------------------------------------

/// Create (or reuse) a run dir + manifest and print `{run_dir}` as JSON. No
/// retrieval — just mints the run so connectors can launch concurrently at t=0.
fn cmd_init_run(query: String, mode: String, out_dir: Option<String>) -> Result<()> {
    let dir = ensure_run(&query, &mode, out_dir.as_deref(), None)?;
    let out = serde_json::json!({ "run_dir": dir.to_string_lossy() });
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

/// Project a run's sources.jsonl into a compact JSON array, sorted by credibility
/// (desc). Deterministic, disk-truthful — the orchestrator triages/reads off this
/// instead of parsing JSONL itself, so selection and counts can't drift from disk.
fn cmd_list_sources(dir: String) -> Result<()> {
    let sources: Vec<Source> = store::read_jsonl(PathBuf::from(&dir).join("sources.jsonl"))?;
    let mut projected: Vec<serde_json::Value> = sources
        .iter()
        .map(|s| {
            let cred = s.credibility.as_ref();
            serde_json::json!({
                "source_id": s.source_id,
                "raw_url": s.raw_url,
                "title": s.title,
                "origin": s.origin,
                "date": s.date,
                "year": s.year,
                "score": cred.map(|c| c.overall).unwrap_or(0.0),
                "recommendation": cred.map(|c| c.recommendation.clone()).unwrap_or_default(),
                "snippet": s.snippet,
            })
        })
        .collect();
    projected.sort_by(|a, b| {
        let sb = b["score"].as_f64().unwrap_or(0.0);
        let sa = a["score"].as_f64().unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    let out = serde_json::json!({
        "dir": dir,
        "count": projected.len(),
        "sources": projected,
    });
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// register-source / add-evidence / add-claim
// ---------------------------------------------------------------------------

fn cmd_register_source(dir: String, json: String) -> Result<()> {
    let source: Source = serde_json::from_str(&json).context("parse source json")?;
    let id = store::register_source(&PathBuf::from(dir), source)?;
    println!("{id}");
    Ok(())
}

fn cmd_add_evidence(dir: String, json: String) -> Result<()> {
    let evidence: Evidence = serde_json::from_str(&json).context("parse evidence json")?;
    let id = store::add_evidence(&PathBuf::from(dir), evidence)?;
    println!("{id}");
    Ok(())
}

fn cmd_add_claim(dir: String, json: String) -> Result<()> {
    let claim: Claim = serde_json::from_str(&json).context("parse claim json")?;
    let id = store::add_claim(&PathBuf::from(dir), claim)?;
    println!("{id}");
    Ok(())
}

// ---------------------------------------------------------------------------
// verify-citations
// ---------------------------------------------------------------------------

async fn cmd_verify_citations(
    dir: Option<String>,
    report: Option<String>,
    strict: bool,
) -> Result<()> {
    let mut entries: Vec<CitationEntry> = Vec::new();

    if let Some(dir) = &dir {
        let sources: Vec<Source> = store::read_jsonl(PathBuf::from(dir).join("sources.jsonl"))?;
        for s in sources {
            let doi = s
                .canonical_locator
                .strip_prefix("doi:")
                .map(|d| d.to_string());
            entries.push(CitationEntry {
                num: None,
                title: Some(s.title.clone()),
                year: s.year.clone(),
                doi,
                url: Some(s.raw_url.clone()),
            });
        }
    }

    if let Some(report) = &report {
        let text = std::fs::read_to_string(report)
            .with_context(|| format!("read report {report}"))?;
        entries.extend(parse_bibliography(&text));
    }

    let client = http::client();
    let futures = entries
        .iter()
        .map(|e| verify::citations::verify(&client, e))
        .collect::<Vec<_>>();
    let verdicts = join_all(futures).await;

    println!("{}", serde_json::to_string_pretty(&verdicts)?);

    if strict {
        let bad = verdicts
            .iter()
            .any(|v| v.status == "suspicious" || v.status == "unverified");
        if bad {
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Parse a `## Bibliography` section of a markdown report into citation rows.
/// Each non-empty line under the heading (until the next `## ` heading) is one
/// entry: a leading `[n]`/`n.` is the num; the first `http(s)://...` token is
/// the url; a `doi:`/`doi.org/` token is the doi; a 4-digit `(YYYY)`/`YYYY` is
/// the year; the remaining text (minus those) is the title.
fn parse_bibliography(text: &str) -> Vec<CitationEntry> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("##") {
            // a new heading; are we entering or leaving the bibliography?
            let heading = rest.trim_start_matches('#').trim().to_ascii_lowercase();
            in_section = heading == "bibliography";
            continue;
        }
        if !in_section || trimmed.is_empty() {
            continue;
        }
        if let Some(entry) = parse_bib_line(trimmed) {
            out.push(entry);
        }
    }
    out
}

fn parse_bib_line(line: &str) -> Option<CitationEntry> {
    // strip a list marker
    let mut s = line
        .trim_start_matches(['-', '*', '+'])
        .trim()
        .to_string();

    // leading number: "[12]" or "12." or "12)"
    let mut num = None;
    if let Some(rest) = s.strip_prefix('[') {
        if let Some(idx) = rest.find(']') {
            let inner = &rest[..idx];
            if inner.chars().all(|c| c.is_ascii_digit()) && !inner.is_empty() {
                num = Some(inner.to_string());
                s = rest[idx + 1..].trim().to_string();
            }
        }
    } else {
        let lead: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !lead.is_empty() {
            let after = &s[lead.len()..];
            if after.starts_with('.') || after.starts_with(')') {
                num = Some(lead);
                s = after[1..].trim().to_string();
            }
        }
    }

    // url + doi
    let mut url = None;
    let mut doi = None;
    let mut remaining_tokens: Vec<String> = Vec::new();
    for tok in s.split_whitespace() {
        let clean = tok.trim_matches(|c| c == '<' || c == '>' || c == '(' || c == ')' || c == ',');
        let lower = clean.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            if lower.contains("doi.org/10.") && doi.is_none() {
                doi = Some(model::canonical_locator(clean).trim_start_matches("doi:").to_string());
            }
            if url.is_none() {
                url = Some(clean.to_string());
            }
        } else if lower.starts_with("doi:") && doi.is_none() {
            doi = Some(clean[4..].to_string());
        } else {
            remaining_tokens.push(tok.to_string());
        }
    }

    // year: first standalone, plausible 4-digit token (optionally parenthesized).
    // Strip surrounding punctuation, then require EXACTLY four contiguous digits in
    // a sane range — so URL/article ids ("Article 2326") and grouped figures
    // ("6,472 hoteliers") stay in the title instead of being mistaken for a year.
    let current_year: i64 = chrono::Local::now()
        .format("%Y")
        .to_string()
        .parse()
        .unwrap_or(0);
    let mut year = None;
    let mut title_tokens: Vec<String> = Vec::new();
    for tok in &remaining_tokens {
        let cleaned = tok.trim_matches(|c: char| !c.is_ascii_digit());
        let is_year = year.is_none()
            && cleaned.len() == 4
            && cleaned.chars().all(|c| c.is_ascii_digit())
            && cleaned
                .parse::<i64>()
                .map(|y| (1900..=current_year + 1).contains(&y))
                .unwrap_or(false);
        if is_year {
            year = Some(cleaned.to_string());
        } else {
            title_tokens.push(tok.clone());
        }
    }

    let title = {
        let t = title_tokens.join(" ").trim().trim_matches('.').trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    };

    if num.is_none() && title.is_none() && url.is_none() && doi.is_none() {
        return None;
    }
    Some(CitationEntry {
        num,
        title,
        year,
        doi,
        url,
    })
}

// ---------------------------------------------------------------------------
// verify-support
// ---------------------------------------------------------------------------

fn cmd_verify_support(dir: String, strict: bool) -> Result<()> {
    let dir = PathBuf::from(dir);
    let mut claims: Vec<Claim> = store::read_jsonl(dir.join("claims.jsonl"))?;
    let evidence: Vec<Evidence> = store::read_jsonl(dir.join("evidence.jsonl"))?;

    // index evidence by evidence_id and by source_id
    let mut by_evidence_id: HashMap<String, &Evidence> = HashMap::new();
    let mut by_source_id: HashMap<String, Vec<&Evidence>> = HashMap::new();
    for e in &evidence {
        by_evidence_id.insert(e.evidence_id.clone(), e);
        by_source_id.entry(e.source_id.clone()).or_default().push(e);
    }

    let mut status_counts: HashMap<String, usize> = HashMap::new();
    let mut total_factual = 0usize;
    let mut factual_unsupported = 0usize;

    for claim in &mut claims {
        let mut quotes: Vec<String> = Vec::new();
        for eid in &claim.evidence_ids {
            if let Some(e) = by_evidence_id.get(eid) {
                quotes.push(e.quote.clone());
            }
        }
        for sid in &claim.cited_source_ids {
            if let Some(list) = by_source_id.get(sid) {
                for e in list {
                    quotes.push(e.quote.clone());
                }
            }
        }

        let result = verify::support::compute(&claim.text, &quotes);
        claim.support_status = result.status.clone();
        *status_counts.entry(result.status.clone()).or_insert(0) += 1;

        if claim.claim_type == model::ClaimType::Factual {
            total_factual += 1;
            if !is_supported(&result.status) {
                factual_unsupported += 1;
            }
        }
    }

    store::write_claims(&dir, &claims)?;

    let summary = serde_json::json!({
        "counts_by_status": status_counts,
        "total_factual": total_factual,
        "factual_unsupported": factual_unsupported,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);

    if strict && factual_unsupported > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn is_supported(status: &str) -> bool {
    matches!(status, "supported" | "strong" | "partial")
}

// ---------------------------------------------------------------------------
// score
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct ScoreInput {
    url: String,
    title: String,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    author: Option<String>,
}

fn cmd_score(json: String) -> Result<()> {
    let input: ScoreInput = serde_json::from_str(&json).context("parse score json")?;
    let trust = verify::credibility::TrustConfig::load();
    let cred = verify::credibility::score_with(
        &input.url,
        &input.title,
        input.date.as_deref(),
        input.author.as_deref(),
        &trust,
    );
    println!("{}", serde_json::to_string_pretty(&cred)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_bib_line;

    // An article id inside the title ("Article 2326") must NOT be read as the
    // year; it stays in the title, and the year is left unset (no real year here).
    #[test]
    fn article_id_in_title_is_not_a_year() {
        let line = "2. Airbnb Help Center, Article 2326, \"Occupancy tax in Puerto Rico.\" https://www.airbnb.co.nz/help/article/2326 — Tier 4, primary.";
        let e = parse_bib_line(line).expect("entry");
        assert_eq!(e.num.as_deref(), Some("2"));
        assert_eq!(e.year, None, "article id 2326 must not be parsed as a year");
        assert!(e.title.as_deref().unwrap().contains("Article 2326"));
    }

    // A grouped figure ("6,472 hoteliers") must NOT be read as the year; the real
    // parenthesized year ("(summer 2024)") later in the line is the one extracted.
    #[test]
    fn grouped_figure_is_not_a_year_and_real_year_wins() {
        let line = "41. CPI — 6,472 hoteliers / 11,398 rooms (summer 2024). https://example.org/a";
        let e = parse_bib_line(line).expect("entry");
        assert_eq!(e.year.as_deref(), Some("2024"));
        assert!(e.title.as_deref().unwrap().contains("6,472 hoteliers"));
    }

    // A plain parenthesized year is still extracted and stripped from the title.
    #[test]
    fn parenthesized_year_extracted() {
        let line = "[5] Some Report (2023). https://example.org/b";
        let e = parse_bib_line(line).expect("entry");
        assert_eq!(e.year.as_deref(), Some("2023"));
        assert_eq!(e.num.as_deref(), Some("5"));
    }
}
