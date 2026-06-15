# research — terminal deep-research

A terminal-native deep-research tool. It fans out across web breadth (Perplexity), X/social
(Grok), and free sources, then **reads, verifies, and synthesizes a cited report locally inside
your Claude Code session** — so the finished report lands in the terminal's context, not in a
web app you have to copy out of.

It is two pieces:

1. **`research`** — a small, dependency-light Rust binary: the deterministic substrate. It owns
   retrieval (Perplexity, Hacker News, GitHub, Polymarket, Grok, OpenAlex, Crossref), URL
   canonicalization and dedup, credibility scoring, a phantom-citation guard (DOI/URL resolution
   + hallucination-pattern checks), and a claim↔evidence support check. It writes append-only
   `sources.jsonl` / `evidence.jsonl` / `claims.jsonl` to a run directory.
2. **A Claude Code skill (`/research`)** — the reasoning layer. It classifies the question,
   routes it to the right modalities, drives the binary, reads the surviving sources, runs an
   iterative search→read→verify→confirm loop, and writes the cited markdown report. The judgment
   and synthesis run on **your** Claude Code subscription.

The design principle: **buy breadth, own synthesis.** Perplexity's index supplies raw ranked
results cheaply; the loop and the writing happen locally, where the report can land in context
and be reasoned over further.

---

## ⚠️ Read this first — what this actually is

**This is a highly personalized tool that I built for my own daily workflow, and it is
battle-tested in that workflow — but it is not a polished product.** It assumes you work the way
I do: inside Claude Code, from a terminal, comfortable wiring API keys and reading shell. It
makes opinionated choices, hard-codes a sensible default source-routing table, and expects you to
adapt it rather than configure it through a UI. It works, and it works well for me. If you pick
it up, expect to read the code and bend it to your setup.

Concretely, it **requires [Claude Code](https://claude.com/claude-code)** — the synthesis layer
*is* a Claude Code skill. Without Claude Code you have a retrieval/verification binary and no
reasoning layer.

---

## Requirements

- **Claude Code** (required — the `/research` skill is the reasoning layer).
- **Rust toolchain** (`cargo`) to build the binary.
- **API keys are optional and additive** — see the capability matrix below. Nothing but the free
  connectors works with zero keys; the tool degrades gracefully as keys are absent.

## Install

```sh
git clone <this-repo> research && cd research
./install.sh                      # default build (Sci-Hub OFF)
# or: ./install.sh --features scihub   # opt into the Sci-Hub full-text reading aid
```

`install.sh` builds the release binary, symlinks it to `~/.local/bin/research`, and installs the
skill into `~/.claude/skills/research/` (honoring `CLAUDE_CONFIG_DIR`). Make sure `~/.local/bin`
is on your `PATH`. Then, in Claude Code:

```
/research <your question>
```

## Capability matrix — what works with which keys

The tool is built to degrade, not break. Every row below is additive.

| You provide | You get |
|---|---|
| **Nothing** | Hacker News + GitHub + Polymarket + local reading/synthesis. (Web breadth degrades to Claude's built-in web search.) |
| `PERPLEXITY_API_KEY` | The core: broad web retrieval with ranked results + excerpts. ($5 / 1,000 search requests.) |
| A local `grok` CLI (xAI SuperGrok) | X/Twitter **and** Reddit social search ($0 marginal on the subscription). Without it, social is skipped — cleanly, not fatally. *(An API-key fallback via `XAI_API_KEY` / `OPENROUTER_API_KEY` is on the roadmap so social works without the CLI.)* |
| `OPENALEX_API_KEY` / `OPENALEX_MAILTO` / `CROSSREF_MAILTO` | Scholarly discovery (OpenAlex + Crossref). All optional; OpenAlex works keyless for light use. |
| `--features scihub` build | A full-text **reading aid** for paywalled papers (cite the DOI, never the mirror). Off by default. |

Modes: **quick** (fast, exploratory) and **deep** (high-stakes — law, health, finance, safety,
major decisions; slow, full iterative verification). Override with `--quick` / `--deep`.

## Configuration

- **API keys** are read from the environment (e.g. `export PERPLEXITY_API_KEY=...`). Keep them in
  a secret manager and export at session start if you prefer.
- **`trust.conf`** (`~/.config/research/trust.conf`) lets you override the built-in domain
  credibility tiers with your own curation — `[trusted]` / `[independent]` / `[distrusted]`. The
  compiled defaults are deliberately neutral (mainstream primary sources); your worldview lives in
  this file, never in the binary. See [`config/trust.conf.example`](config/trust.conf.example).
- **Sci-Hub** support is compiled only with `--features scihub`. When built in, it's a personal
  reading aid: it fetches a paper's bytes so you can read them, but **the citation always points
  to the DOI/publisher, never the mirror**. The corpus is frozen at 2021, so a miss on a newer
  paper is expected, not a bug. See [`config/scihub.conf.example`](config/scihub.conf.example).

## Costs

Reading and synthesis run on your Claude Code subscription. Beyond that, the only paid component
is retrieval: Perplexity's Search API is ~$5 per 1,000 requests (no token charge). The local
`grok` CLI is $0-marginal on a SuperGrok subscription; the free connectors cost nothing.

## Limitations (honest list)

- Requires Claude Code; there is no standalone CLI report generator.
- The source-routing table and credibility tiers reflect my judgment; tune them to yours.
- Social currently needs the local `grok` CLI (API-key fallback is planned).
- Reddit coverage rides on Grok and can be patchy.
- This is a personal tool shared as-is — issues and PRs welcome, but support is best-effort.

## License

[MIT](LICENSE).
