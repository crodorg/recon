#!/bin/sh
# install.sh — build the `research` binary and install the /research Claude Code skill.
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
SKILL_DIR="${CLAUDE_DIR}/skills/research"

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
echo ">> building research (release${FEATURES:+, features: $FEATURES})"
if [ -n "$FEATURES" ]; then
	(cd "$REPO" && cargo build --release --features "$FEATURES")
else
	(cd "$REPO" && cargo build --release)
fi

# --- install binary ---------------------------------------------------------
mkdir -p "$BIN_DIR"
ln -sf "$REPO/target/release/research" "$BIN_DIR/research"
echo ">> linked $BIN_DIR/research -> $REPO/target/release/research"

# --- install skill (token-substituted to the real install path) ------------
mkdir -p "$SKILL_DIR"
for f in SKILL.md research.workflow.js sources.md query-strategy.md; do
	sed "s|__RESEARCH_SKILL_DIR__|$SKILL_DIR|g" "$REPO/skill/$f" >"$SKILL_DIR/$f"
done
echo ">> installed skill to $SKILL_DIR"

# --- post-install notes -----------------------------------------------------
cat <<EOF

Done. Next steps:
  * Ensure $BIN_DIR is on your PATH.
  * Export PERPLEXITY_API_KEY for web breadth (the core connector).
  * Optional: a local 'grok' CLI (xAI SuperGrok) enables X/Reddit social search;
             OPENALEX_API_KEY / *_MAILTO for scholarly discovery.
  * In Claude Code, run:  /research <your question>

The free connectors (HN, GitHub, Polymarket) and local synthesis work with no keys.
See README.md for the full capability matrix.
EOF
