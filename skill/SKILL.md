---
name: research
description: "Terminal deep-research: routes a query to the right sources (web via Perplexity, X/social via Grok, repos/HN/markets), reads + verifies locally, writes a cited markdown report into the session. Invoke: '/research <query>'. Two modes — quick (fast, exploratory) and deep (high-stakes: law, health, finance, safety, major decisions; slow, full verification). Flags: --deep / --quick. Not for trivial lookups you can answer directly."
---

# research — terminal deep-research

Approaches claude.ai-Research depth (many sources → a cited report) but runs from the
terminal, so the report lands in this session's context. **Perplexity owns web breadth;
Grok owns X/social; you (local Claude, on the subscription) own reading, verification, and
synthesis.** The `research` binary is the deterministic substrate — retrieval, dedup,
credibility scoring, phantom-citation and claim-support checks. You supply the judgment.

The one rule that makes this good: **route to the modality the answer lives in.** Don't
ask a social feed about a statute; don't ask the U.S. Code whether a laptop is worth
buying. The routing table is `sources.md` — read it; it is the source of truth for
connector choice.

---

## Step 0 — preflight

1. **Key.** Perplexity reads `PERPLEXITY_API_KEY` from the environment. Make sure it's set
   for this session:
   ```sh
   PPLX="${PERPLEXITY_API_KEY}"   # export it in your shell, or have your secret manager set it
   ```
   Prefix every `research retrieve` with `PERPLEXITY_API_KEY="$PPLX"`. If it's empty, tell
   the user to export the key, and offer to proceed keyless (Perplexity drops out; Grok/X +
   free sources + local synthesis still run).
2. **Binary.** `command -v research` — if missing, the user needs to install it: run
   `./install.sh` from the repo (builds the release binary, symlinks it to
   `~/.local/bin/research`, and installs this skill). See the repo README.
3. The binary creates and owns the run dir under `~/.local/share/research/runs/<id>/`
   (`sources.jsonl` / `evidence.jsonl` / `claims.jsonl` / `run_manifest.json`). You write
   the final report into that same dir.

## Step 1 — classify

Parse the query and any flags. Decide two things:

- **Profile** (which connectors) — map intent → profile per `sources.md` (authoritative /
  technical / social / markets / general). Authoritative suppresses social.
- **Mode** (how hard the local loop works):
  - `--deep` flag, or explicit deep-research language ("do deep research", "thoroughly",
    "exhaustive") → **deep**.
  - `--quick` flag, or laid-back / exploratory phrasing → **quick**.
  - Otherwise infer from **stakes**: money, health, finance, legal/regulatory, safety,
    emergencies, mental health, or a major irreversible decision → lean **deep**;
    everything else → **quick**.
- **Tier** (deep only — sets the query + read budget the workflow uses):
  - `--deep` flag, or top-stakes (legal/health/finance/safety/major irreversible decision),
    or a broad multi-facet question → **max** (~32 queries, ~22–25 full reads, 3 rounds).
  - Any other deep run → **standard** (~22 queries, ~14–16 reads, 3 rounds).
  - Quick mode is its own light path (inline, below); it does not use the workflow.
- **Primary-tracing** (`trace_primary`, deep only) — set **true** whenever the question has a
  legal / regulatory / health / finance core, *even when social stays ON* (`general`/`social`
  profile). It is independent of social suppression: it makes the read + synthesis agents trace
  load-bearing claims to the primary (statute / filing / dataset), not stop at a blog about it.
  `authoritative` profile implies it; pure opinion/consumer questions → false.

## Step 2 — gate the run

- **Quick** → just run it.
- **Deep, explicitly requested** (flag or clear language) → just run it; print one line
  first: `Classified: deep / <profile> → sources: <list> (no social if authoritative).`
- **Deep, inferred** (you decided it's high-stakes but the user didn't say so) → **state
  the plan and confirm before firing**: the mode, profile, connector list, that it spawns
  a parallel read/verify workflow, and the rough shape ("dozens of sources, a few
  minutes"). Wait for go-ahead, then proceed and finish normally. Don't surprise the user
  with a long, Perplexity-spending run they didn't ask for.

## Step 3 — route

Translate the profile into the `--sources` split (`sources.md`). Split **fast vs slow** so
reading can start before the slow social connector returns:

- `fast_sources` — the quick HTTP connectors: `perplexity`, plus `hn`/`github` if technical.
- `slow_sources` — `grok` (X/social, ~10–120s) when the profile includes social; else empty.

Examples: authoritative → fast `perplexity`, slow ``. technical → fast
`perplexity,hn,github`, slow `grok`. social → fast `perplexity`, slow `grok,reddit` (X +
Reddit, both Grok shell-outs). markets → fast `perplexity,polymarket`, slow `grok`.

**Scholarly augmentation:** when the query has a health / medical / scientific / academic
core (any profile), add `openalex,crossref` to `fast_sources` — they surface the primary
literature (DOIs, abstracts) web breadth misses. In deep mode the read step then
auto-fetches full text via Sci-Hub for load-bearing paywalled papers **if the binary was
built with `--features scihub`** (cite the DOI; see the full-text section below). OpenAlex
works keyless for light use; a free `OPENALEX_API_KEY` (env) lifts the daily ceiling.

## Step 4 — retrieve, read, verify

### Quick mode (inline — you do it here)

1. **Retrieve, staged.** Fire the fast batch; capture `run_dir`:
   ```sh
   RUN=$(PERPLEXITY_API_KEY="$PPLX" research retrieve "<query>" --mode quick \
          --sources <fast_sources> --limit 8 | tee /tmp/r-fast.json \
          | python3 -c 'import sys,json;print(json.load(sys.stdin)["run_dir"])')
   ```
   If `slow_sources` is non-empty, append it into the same run (it runs while you read):
   ```sh
   PERPLEXITY_API_KEY="$PPLX" research retrieve "<query>" --mode quick \
     --sources <slow_sources> --limit 6 --run-dir "$RUN" >/tmp/r-slow.json 2>/dev/null &
   ```
2. **Read** the top ~6–10 sources by credibility (read `$RUN/sources.jsonl`). For each,
   `web_fetch` the URL, pull the load-bearing facts. For authoritative queries, follow the
   primary-source hierarchy in `sources.md` — trace claims to primary text, don't stop at
   a blog about the statute. `wait` for the slow batch, then fold in the social signal.
3. **Write** the report (Step 5). Then **verify**: `research verify-citations --dir "$RUN"`
   (live URL/DOI checks). Fix or flag anything `suspicious`/`unverified`.

Quick output shape: a **Research Brief** — verdict up top, then findings with inline `[N]`
citations, a short bibliography, confidence label, as-of date. Concise; no fluff.

### Deep mode (the workflow — the iterative engine)

Invoke the **Workflow tool** with the installed deep orchestrator. It runs the full
iterative engine: **decompose** into a 6-axis query set → **Round 1** broad retrieve
(Perplexity queries concurrent with Grok-X at t=0) → **triage** on excerpt → **read** the
best survivors → **gap critic** → **Round 2** gap-driven deepening (primary-source tracing)
→ **Round 3** narrow confirmation of the load-bearing claims → **synthesize** a cited report
+ verify. It retrieves WIDE and reads SELECTIVELY — depth comes from query decomposition and
iteration, not a bigger single call. Doctrine lives in `query-strategy.md` (read by the
strategist) and `sources.md` (routing + tiers + writing contract).

Before invoking, **front-load context**: if the query references a local project or file
(e.g. `../str`), read it first and pass the salient brief as `context` so the strategist
doesn't burn queries rediscovering what you already have.

```
Workflow({
  scriptPath: "__RESEARCH_SKILL_DIR__/research.workflow.js",
  args: {
    query: "<query>",
    context: "<front-loaded known context, or ''>",
    profile: "<profile>",             // authoritative / technical / social / markets / general
    tier: "<standard|max>",           // from Step 1 (--deep → max)
    trace_primary: <true|false>,      // true for any legal/regulatory/health/finance core, even with social ON
    fast_sources: "<fast_sources>",
    slow_sources: "<slow_sources>",   // "" if authoritative (social suppressed)
    mode: "deep",
    as_of: "<today YYYY-MM-DD>",       // pass it; the workflow can't read the clock
    skill_dir: "__RESEARCH_SKILL_DIR__"
  }
})
```

The workflow returns (all counts disk-derived): `{ run_dir, report_path, tier, queries_fired,
sources_total, sources_read, sources_reachable, sources_inaccessible, evidence_rows, claims,
verified, load_bearing_claims, gaps_remaining, limitations, summary }`. Read `report_path` into the
session and present it — in chat give the verdict, the path, and the headline caveats. If
`verified` shows unresolved failures, or `gaps_remaining` / `limitations` are non-empty,
**surface them** — don't bury a failed check or an open gap.

## Step 5 — output (the writing contract)

Write a cited **markdown** report to `$RUN/report.md` and present it in-session (no
PDF/HTML — the point is that it lands in the terminal context). Both shapes obey:

- **≥80% prose.** Explain and argue; don't dump bullet lists of disconnected facts.
- **Immediate `[N]` citations** at the point of claim. Every `[N]` resolves to a
  bibliography entry with a real URL/DOI.
- **Zero-tolerance bibliography:** no ranges, no placeholders, no "various sources". Each
  entry is one real, checkable source, labeled by tier (`sources.md` ladder).
- **No vague attribution:** never "studies show" / "experts say" without the specific
  source behind it.
- **Precision over hedging.** State what the evidence supports and the confidence (High /
  Medium / Low / Unverified). **Surface conflicts** between sources rather than averaging
  them away. Label each source primary / secondary / social.
- **Stamp the research-as-of date** and note what was and wasn't checked.

Keep the chat reply tight: the report file is the deliverable; in chat give the verdict,
the path, and the headline caveats.

---

## Full-text retrieval (Sci-Hub) — a reading aid, NOT a citation source

> **Opt-in feature, off by default.** Sci-Hub support is compiled only when the binary is
> built with `cargo build --features scihub`. Without it, `research fetch-paper` doesn't
> exist and the deep read step simply skips this rung — a blocked paper becomes an
> `inaccessible` gap marker (below). Everything else works unchanged.

**Auto-routed in deep reads (Phase 9):** the read agent calls this itself when a
load-bearing source is a paywalled academic paper carrying a DOI/PMID — fetch full text →
read the PDF → cite the DOI. Conservative (load-bearing only, no fan-out, 2022+ misses
expected). The manual call below is for one-offs (quick mode, or chasing a specific paper).

**When a load-bearing primary can't be read in full, surface the gap — don't bury it.** Two
cases: an academic paper paywalled AND not in the sci-hub corpus (`found:false`), OR any other
primary (statute / filing / .gov / PDF / dataset) that blocked every fetch rung (direct + Grok).
In quick mode, notify in the brief; in deep mode the read step records a `provenance:"inaccessible"`
marker that synthesis aggregates and the workflow returns as `sources_inaccessible` (the miss
count, now surfaced rather than dropped silently). **Aggregate, don't spam:** one inaccessible
primary → a single plain notice; many → say what happened in net ("N primaries on X were
inaccessible; the M I read agree on X, so the verdict holds" — or, if a claim rested only on the
inaccessible ones, name them and downgrade confidence). Never list one line per missed source.

When the user wants the **actual full text** of a specific paper (typically paywalled, e.g.
chasing a PubMed hit down to the paper itself), fetch it:

```sh
research fetch-paper <doi|pmid> --out <dir>   # 10.1038/171737a0  |  pmid:13054692  |  123456
```

Returns JSON: `{ found, doi, pmid?, pdf_path?, domain_used?, domains_tried[], note? }`. On
`found:true`, read `pdf_path` for the content (the `pdf` skill / `pdftotext`). It's keyed by
DOI; a PMID is auto-resolved via NCBI eutils. The live-domain mirror self-refreshes (24h TTL;
`--refresh` forces a re-probe; `research scihub-domains` inspects it). If every domain is dead,
tell the operator to add a current one to `~/.config/research/scihub.conf` and re-run `--refresh`.

**Hard rules — keep it walled off from the cite flow:**

- **Cite the DOI/publisher, NEVER the Sci-Hub mirror.** `pdf_path`/`domain_used` are local
  read artifacts — they never go into a `## Bibliography`, a `Source`, or `verify-citations`.
  This is the same doctrine as `query-strategy.md` ("never cite a piracy mirror URL"); the
  fetcher just gets you the bytes to read.
- **Corpus is frozen at 2021.** Sci-Hub has added nothing since (pending litigation), so a
  miss on a 2022+ paper is *expected, not a bug* — surface the `note`, don't retry in a loop.
- It's a personal-access reading aid. Don't fan it out across many DOIs reflexively; fetch the
  specific paper the user actually needs.
