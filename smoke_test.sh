#!/usr/bin/env bash
# Compare crabase output against the real obsidian CLI across an entire vault.
# For each .base file and each named view, runs `base:query format=csv` in both
# and diffs the result. Also diffs the `bases` listing.
#
# Usage: smoke_test.sh <vault_path>

set -uo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <vault_path>" >&2
  exit 2
fi

VAULT="$1"

if [[ ! -d "$VAULT" ]]; then
  echo "Vault path does not exist: $VAULT" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v obsidian >/dev/null 2>&1; then
  echo "obsidian CLI not found in PATH" >&2
  exit 1
fi

echo "Building crabase..."
(cd "$SCRIPT_DIR" && cargo build --release --quiet) || { echo "crabase build failed"; exit 1; }
CRABASE="$SCRIPT_DIR/target/release/crabase"

if [[ -t 1 ]]; then
  RED=$'\033[0;31m'; GREEN=$'\033[0;32m'; YELLOW=$'\033[1;33m'
  BOLD=$'\033[1m'; DIM=$'\033[2m'; NC=$'\033[0m'
else
  RED=''; GREEN=''; YELLOW=''; BOLD=''; DIM=''; NC=''
fi

PASS=0; FAIL=0; SKIP=0
FAILURES=()

pass() { printf '%sPASS%s %s\n' "$GREEN" "$NC" "$1"; PASS=$((PASS+1)); }
skip() { printf '%sSKIP%s %s%s%s\n' "$YELLOW" "$NC" "$DIM" "$1" "$NC"; SKIP=$((SKIP+1)); }
fail() {
  printf '%sFAIL%s %s\n' "$RED" "$NC" "$1"
  FAIL=$((FAIL+1))
  FAILURES+=("$1")
  if [[ -n "${2:-}" ]]; then
    printf '%s%s%s\n' "$DIM" "$2" "$NC"
  fi
}

# Show a compact diff: header + at most ~20 lines.
short_diff() {
  local a="$1" b="$2"
  diff -u --label=obsidian --label=crabase <(printf '%s' "$a") <(printf '%s' "$b") \
    | sed -e 's/^/    /' \
    | head -n 22
}

# ---------- 1. Compare base listings ----------
printf '\n%sListing bases%s\n' "$BOLD" "$NC"
OBS_BASES=$(obsidian bases vault="$VAULT" 2>/dev/null | LC_ALL=C sort)
CRA_BASES=$("$CRABASE" bases vault="$VAULT" 2>/dev/null | LC_ALL=C sort)

if [[ "$OBS_BASES" == "$CRA_BASES" ]]; then
  pass "bases listing"
else
  fail "bases listing" "$(short_diff "$OBS_BASES" "$CRA_BASES")"
fi

# ---------- 2. For each base × view, compare query output ----------
printf '\n%sQuerying each (base, view)%s\n' "$BOLD" "$NC"

# Iterate the intersection of both listings — bases unique to one side already
# show up in the listing diff above.
COMMON_BASES=$(comm -12 <(printf '%s\n' "$OBS_BASES") <(printf '%s\n' "$CRA_BASES"))

while IFS= read -r base; do
  [[ -z "$base" ]] && continue

  views=$("$CRABASE" base:views file="$base" vault="$VAULT" 2>/dev/null)
  if [[ -z "$views" ]]; then
    skip "$base (no views)"
    continue
  fi

  while IFS= read -r view; do
    [[ -z "$view" ]] && continue
    label="$base :: $view"

    if [[ "$view" == "(unnamed)" ]]; then
      # No way to address an unnamed view by name on the command line;
      # both CLIs default to the first view when view= is omitted, so test that.
      obs_out=$(obsidian base:query file="$base" format=csv vault="$VAULT" 2>/dev/null)
      cra_out=$("$CRABASE" base:query file="$base" format=csv vault="$VAULT" 2>/dev/null)
    else
      obs_out=$(obsidian base:query file="$base" format=csv view="$view" vault="$VAULT" 2>/dev/null)
      cra_out=$("$CRABASE" base:query file="$base" format=csv view="$view" vault="$VAULT" 2>/dev/null)
    fi

    if [[ "$obs_out" == "$cra_out" ]]; then
      pass "$label"
    else
      fail "$label" "$(short_diff "$obs_out" "$cra_out")"
    fi
  done <<< "$views"
done <<< "$COMMON_BASES"

# ---------- Summary ----------
printf '\n%sSummary:%s %s%d passed%s, %s%d failed%s, %s%d skipped%s\n' \
  "$BOLD" "$NC" "$GREEN" "$PASS" "$NC" "$RED" "$FAIL" "$NC" "$YELLOW" "$SKIP" "$NC"

if (( FAIL > 0 )); then
  printf '%sFailures:%s\n' "$BOLD" "$NC"
  for f in "${FAILURES[@]}"; do
    printf '  - %s\n' "$f"
  done
  exit 1
fi
