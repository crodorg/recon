//! X/Twitter + web connector backed by the local agentic `grok` CLI.
//!
//! Auth is the operator's SuperGrok subscription wired into `~/.local/bin/grok`
//! (no API key / env var). We run a single non-interactive turn-limited prompt
//! asking Grok to search X (via its `x_keyword_search` / `x_semantic_search` /
//! `web_search` tools) and emit a STRICT JSON array of `{text,author,url,date}`.
//! Grok sometimes wraps prose around that array, so we extract the LAST balanced
//! JSON array in stdout and parse it. The whole call is wrapped in a hard
//! timeout; failure/timeout surfaces as `Err` with detail.

use crate::model::{Candidate, SourceType};
use reqwest::Client;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// Path to the grok CLI. `~` is expanded against `$HOME` at call time.
const GROK_REL_PATH: &str = ".local/bin/grok";

/// Hard wall-clock budget for the whole agentic run (X search). Bumped 120->240s
/// 2026-06-11: a live deep run timed out at 120s and returned ZERO raw voices on a
/// sentiment question; a retry came back in 92s — borderline against the old ceiling.
/// Grok-X's search+fetch loop overruns 120s the same way Reddit does, so match it.
const RUN_TIMEOUT: Duration = Duration::from_secs(240);

/// Reddit needs longer: Grok does web_search plus several old.reddit.com / PullPush
/// fetches, which routinely overruns the X budget (observed: 120s timeout).
const REDDIT_TIMEOUT: Duration = Duration::from_secs(240);

/// Turn budget handed to grok (agentic tool loop).
const MAX_TURNS: &str = "10";

/// One raw row as Grok is asked to emit it.
#[derive(serde::Deserialize)]
struct Row {
    #[serde(default)]
    text: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    date: Option<String>,
}

/// Search X/Twitter + web via the agentic grok CLI.
///
/// `client` is unused (we shell out, not HTTP); kept to honor the frozen
/// connector signature.
pub async fn search(client: &Client, query: &str, limit: usize) -> anyhow::Result<Vec<Candidate>> {
    let _ = client;
    let want = if limit == 0 { 10 } else { limit };
    run_grok(
        &build_prompt(query, want),
        want,
        "grok-x",
        "https://x.com",
        RUN_TIMEOUT,
    )
    .await
}

/// Search Reddit via the same agentic grok CLI. Our own fetcher is IP-blocked by
/// Reddit, but Grok reaches it through `old.reddit.com` + the PullPush archive API.
/// Social tier, origin `grok-reddit` — human opinion / niche + local knowledge,
/// complementary to X (which leans news/breaking).
pub async fn search_reddit(
    client: &Client,
    query: &str,
    limit: usize,
) -> anyhow::Result<Vec<Candidate>> {
    let _ = client;
    let want = if limit == 0 { 10 } else { limit };
    run_grok(
        &build_reddit_prompt(query, want),
        want,
        "grok-reddit",
        "https://old.reddit.com",
        REDDIT_TIMEOUT,
    )
    .await
}

/// Run one non-interactive, turn-limited grok prompt; parse the last JSON array of
/// rows from stdout; map up to `want` non-empty rows to Candidates tagged with
/// `origin` (`url_fallback` is used when a row omits its URL). Shared by the X and
/// Reddit entry points — they differ only in prompt, origin, and URL fallback.
async fn run_grok(
    prompt: &str,
    want: usize,
    origin: &str,
    url_fallback: &str,
    run_timeout: Duration,
) -> anyhow::Result<Vec<Candidate>> {
    let grok = grok_bin();
    if !grok.exists() {
        anyhow::bail!("grok CLI not found at {}", grok.display());
    }

    // NOTE: deliberately NO --sandbox off / --always-approve / --permission-mode
    // bypassPermissions. Default permission mode is used; grok's built-in search
    // tools (x_keyword_search, web_search, ...) run without filesystem writes.
    let mut cmd = Command::new(&grok);
    cmd.arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("plain")
        .arg("--max-turns")
        .arg(MAX_TURNS)
        .arg("--no-alt-screen")
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());

    let output = match timeout(run_timeout, cmd.output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => anyhow::bail!("failed to spawn grok ({}): {e}", grok.display()),
        Err(_) => anyhow::bail!("grok timed out after {}s", run_timeout.as_secs()),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        let detail = first_nonempty(&stderr, &stdout);
        anyhow::bail!(
            "grok exited with {}: {}",
            output.status,
            truncate(&detail, 500)
        );
    }

    let rows = extract_last_json_array(&stdout).ok_or_else(|| {
        anyhow::anyhow!(
            "no parseable JSON array in grok output (stdout {} bytes): {}",
            stdout.len(),
            truncate(stdout.trim(), 500)
        )
    })?;

    let mut out = Vec::with_capacity(rows.len().min(want));
    for row in rows.into_iter().take(want) {
        // Drop fully-empty rows (Grok occasionally pads with blanks).
        if row.text.trim().is_empty() && row.author.trim().is_empty() && row.url.trim().is_empty() {
            continue;
        }
        out.push(to_candidate(row, origin, url_fallback));
    }

    Ok(out)
}

/// Absolute path to the grok binary (`$HOME/.local/bin/grok`).
fn grok_bin() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(GROK_REL_PATH)
}

/// The instruction handed to Grok. Tight and explicit so the agent returns a
/// bare JSON array we can parse from stdout.
fn build_prompt(query: &str, want: usize) -> String {
    format!(
        "Search X/Twitter for recent posts about: {query}\n\
         Use your x_keyword_search and x_semantic_search tools (and web_search if \
         helpful) to find up to {want} relevant recent posts.\n\
         Then output ONLY a strict JSON array (no markdown fences, no prose before \
         or after) of objects, each with exactly these keys:\n\
         - \"text\": the full post text\n\
         - \"author\": the post author (name and/or @handle)\n\
         - \"url\": the direct URL to the post\n\
         - \"date\": the post date as a string (or null if unknown)\n\
         Return the JSON array as the final thing in your response and nothing else after it."
    )
}

/// The Reddit instruction. Same strict-JSON-array contract as X; differs in where
/// Grok looks (old.reddit.com + PullPush, since the main site blocks bots).
fn build_reddit_prompt(query: &str, want: usize) -> String {
    format!(
        "Search Reddit for relevant discussion about: {query}\n\
         Reddit's main site blocks bots, so reach it via old.reddit.com and/or the \
         PullPush archive API (api.pullpush.io) — those work. Use web_search to find \
         threads, then fetch them. Find up to {want} substantive, on-topic posts or \
         comments (prefer upvoted ones from topic-specific subreddits over generic).\n\
         Then output ONLY a strict JSON array (no markdown fences, no prose before or \
         after) of objects, each with exactly these keys:\n\
         - \"text\": the post/comment text, verbatim (trim if very long)\n\
         - \"author\": the subreddit and/or username (e.g. \"r/PuertoRico u/example\")\n\
         - \"url\": the direct old.reddit.com URL to the post/comment\n\
         - \"date\": the date as a string (or null if unknown)\n\
         Return the JSON array as the final thing in your response and nothing else after it."
    )
}

/// Map a raw Grok row to the frozen `Candidate` shape. `origin` tags the modality
/// (`grok-x` / `grok-reddit`); `url_fallback` is used when the row omits its URL.
fn to_candidate(row: Row, origin: &str, url_fallback: &str) -> Candidate {
    let text = row.text.trim().to_string();
    let author = row.author.trim().to_string();

    let raw_url = {
        let u = row.url.trim();
        if u.is_empty() {
            url_fallback.to_string()
        } else {
            u.to_string()
        }
    };

    // title = author + first words of text.
    let head: String = text
        .split_whitespace()
        .take(12)
        .collect::<Vec<_>>()
        .join(" ");
    let title = match (author.is_empty(), head.is_empty()) {
        (false, false) => format!("{author}: {head}"),
        (false, true) => author.clone(),
        (true, false) => head,
        (true, true) => "post".to_string(),
    };

    let date = row
        .date
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());

    Candidate {
        raw_url,
        title,
        snippet: text,
        date,
        source_type: SourceType::Social,
        origin: origin.to_string(),
        extra: serde_json::json!({ "author": author }),
    }
}

/// Find the LAST balanced, JSON-parseable `[...]` array in `s` and decode it as
/// `Vec<Row>`. Scans `[` positions right-to-left; at each, walks forward
/// tracking bracket depth (string-aware so brackets inside strings don't count)
/// to the matching `]`, then tries to parse. Returns the first success.
fn extract_last_json_array(s: &str) -> Option<Vec<Row>> {
    let bytes = s.as_bytes();
    // Byte offsets of every '[' that is NOT inside a JSON string. Strings can
    // only appear after we've entered an array, so a top-level scan that tracks
    // string state catches all candidate starts correctly.
    let mut starts: Vec<usize> = Vec::new();
    {
        let mut in_str = false;
        let mut escaped = false;
        for (i, &b) in bytes.iter().enumerate() {
            if in_str {
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    in_str = false;
                }
                continue;
            }
            match b {
                b'"' => in_str = true,
                b'[' => starts.push(i),
                _ => {}
            }
        }
    }

    for &start in starts.iter().rev() {
        if let Some(end) = match_array_end(bytes, start) {
            let slice = &s[start..=end];
            if let Ok(rows) = serde_json::from_str::<Vec<Row>>(slice) {
                return Some(rows);
            }
        }
    }
    None
}

/// Given `bytes[start] == b'['`, return the index of the matching `]`, tracking
/// nested brackets and skipping bracket chars inside JSON strings. None if
/// unbalanced.
fn match_array_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// First of two strings that has non-whitespace content (trimmed).
fn first_nonempty(a: &str, b: &str) -> String {
    if !a.trim().is_empty() {
        a.trim().to_string()
    } else {
        b.trim().to_string()
    }
}

/// Truncate to at most `max` chars (char-boundary safe), appending an ellipsis
/// marker when cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str(" ...[truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bare_array() {
        let s = r#"[{"text":"hi","author":"a","url":"https://x.com/1","date":"d"}]"#;
        let rows = extract_last_json_array(s).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "hi");
    }

    #[test]
    fn extracts_array_with_prose_around() {
        let s = "Here are the results I found:\n\
                 [{\"text\":\"first\",\"author\":\"@a\",\"url\":\"https://x.com/1\",\"date\":null}]\n\
                 Let me know if you need more.";
        let rows = extract_last_json_array(s).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].author, "@a");
        assert!(rows[0].date.is_none());
    }

    #[test]
    fn picks_last_array_when_multiple() {
        // An earlier non-row array (e.g. a tool log) then the real one.
        let s = "tool call args: [1,2,3]\n\
                 final: [{\"text\":\"real\",\"author\":\"b\",\"url\":\"u\",\"date\":\"d\"}]";
        let rows = extract_last_json_array(s).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "real");
    }

    #[test]
    fn brackets_inside_strings_dont_break_matching() {
        let s = r#"[{"text":"array [a] inside","author":"x","url":"u","date":null}]"#;
        let rows = extract_last_json_array(s).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "array [a] inside");
    }

    #[test]
    fn maps_to_candidate_with_fallback_url() {
        let row = Row {
            text: "hello world this is a post".to_string(),
            author: "Jane (@jane)".to_string(),
            url: "".to_string(),
            date: Some("2026-01-01".to_string()),
        };
        let c = to_candidate(row, "grok-x", "https://x.com");
        assert_eq!(c.raw_url, "https://x.com");
        assert_eq!(c.origin, "grok-x");
        assert_eq!(c.source_type, SourceType::Social);
        assert!(c.title.starts_with("Jane (@jane):"));
        assert_eq!(c.extra["author"], "Jane (@jane)");
        assert_eq!(c.date.as_deref(), Some("2026-01-01"));
    }

    #[test]
    fn reddit_mapping_uses_reddit_origin_and_fallback() {
        let row = Row {
            text: "Act 60 has been worth it for us after four years".to_string(),
            author: "r/PuertoRico u/example".to_string(),
            url: "".to_string(),
            date: None,
        };
        let c = to_candidate(row, "grok-reddit", "https://old.reddit.com");
        assert_eq!(c.origin, "grok-reddit");
        assert_eq!(c.raw_url, "https://old.reddit.com");
        assert_eq!(c.source_type, SourceType::Social);
        assert!(c.title.starts_with("r/PuertoRico u/example:"));
    }

    #[test]
    fn no_array_returns_none() {
        assert!(extract_last_json_array("just prose, no json here").is_none());
    }
}
