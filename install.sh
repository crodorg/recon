#!/bin/sh
# install.sh — build the `recon` binary and install the /recon Claude Code skill.
#
# Usage:
#   ./install.sh                      # default build (Sci-Hub OFF)
#   ./install.sh --features scihub    # opt into the Sci-Hub full-text reading aid
#
# Honors CLAUDE_CONFIG_DIR (default ~/.claude); installs the binary to ~/.local/bin.
set -eu

usage() {
	sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
}

# --- resolve paths ----------------------------------------------------------
REPO="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
CLAUDE_DIR="${CLAUDE_CONFIG_DIR:-${HOME}/.claude}"
SKILL_DIR="${CLAUDE_DIR}/skills/recon"

# --- parse args (pass-through cargo features) -------------------------------
FEATURES=""
while [ $# -gt 0 ]; do
	case "$1" in
	--features) FEATURES="${2:?--features needs a value}"; shift 2 ;;
	--features=*) FEATURES="${1#--features=}"; shift ;;
	-h | --help) usage; exit 0 ;;
	*) echo "unknown argument: $1" >&2; usage; exit 2 ;;
	esac
done

# --- build ------------------------------------------------------------------
echo ">> building recon (release${FEATURES:+, features: $FEATURES})"
if [ -n "$FEATURES" ]; then
	(cd "$REPO" && cargo build --release --features "$FEATURES")
else
	(cd "$REPO" && cargo build --release)
fi

# --- install binary ---------------------------------------------------------
mkdir -p "$BIN_DIR"
ln -sf "$REPO/target/release/recon" "$BIN_DIR/recon"
echo ">> linked $BIN_DIR/recon -> $REPO/target/release/recon"

# --- install skill (token-substituted to the real install path) ------------
mkdir -p "$SKILL_DIR"
for f in SKILL.md recon.workflow.js sources.md query-strategy.md; do
	sed "s|__RECON_SKILL_DIR__|$SKILL_DIR|g" "$REPO/skill/$f" >"$SKILL_DIR/$f"
done
echo ">> installed skill to $SKILL_DIR"

# --- post-install notes -----------------------------------------------------
cat <<EOF

Done. Next steps:
  * Ensure $BIN_DIR is on your PATH.
  * Export PERPLEXITY_API_KEY for web breadth (the core connector).
  * Optional, for X/Reddit social search: a local 'grok' CLI (xAI SuperGrok, \$0
             marginal) OR an XAI_API_KEY / OPENROUTER_API_KEY fallback (paid per call).
  * Optional: OPENALEX_API_KEY / *_MAILTO for scholarly discovery.
  * In Claude Code, run:  /recon <your question>

The free connectors (HN, GitHub, Polymarket) and local synthesis work with no keys.
See README.md for the full capability matrix.
EOF
