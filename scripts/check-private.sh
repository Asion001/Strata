#!/usr/bin/env bash
# Blocks personal data (real names, emails, domains, hosts, IPs, local paths)
# from entering the repository.
#
# The patterns are private themselves, so they never live in git. They come
# from, in order:
#   1. $PRIVATE_PATTERNS            (newline separated, e.g. a CI secret)
#   2. $PRIVATE_PATTERNS_FILE
#   3. <repo>/.private-patterns     (gitignored)
#   4. ~/.config/strata/private-patterns
# One case-insensitive extended regex per line; '#' comments and blank lines
# are ignored. With no patterns the check skips (exit 0) with a notice.
#
# Usage:
#   check-private.sh                    staged diff + commit identity (pre-commit)
#   check-private.sh --commit-msg FILE  a commit message (commit-msg hook)
#   check-private.sh --range A..B       added lines, messages and authors of A..B (CI)
#   check-private.sh --stdin [LABEL]    text on stdin (PR titles, bodies, tool input)
#   check-private.sh FILE...            whole files
#
# When CI is set, matches are reported by location only, so the private text
# never reaches public logs.
#
# Install the hooks once per clone:  git config core.hooksPath .githooks
set -euo pipefail

root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)

load_patterns() {
  if [[ -n "${PRIVATE_PATTERNS:-}" ]]; then
    printf '%s\n' "$PRIVATE_PATTERNS"
    return
  fi
  local f
  for f in "${PRIVATE_PATTERNS_FILE:-}" "$root/.private-patterns" "$HOME/.config/strata/private-patterns"; do
    if [[ -n "$f" && -f "$f" ]]; then
      cat "$f"
      return
    fi
  done
}

regex=$(load_patterns | tr -d '\r' | grep -v -e '^[[:space:]]*#' -e '^[[:space:]]*$' | paste -sd'|' - || true)
if [[ -z "$regex" ]]; then
  echo "check-private: no private patterns configured, skipping" >&2
  exit 0
fi

status=0

# report LABEL  (reads text on stdin, prints matches, sets status).
# Feed it with a redirect, not a pipe, so the status survives.
report() {
  local label="$1" hits
  hits=$(grep -n -i -E -- "$regex" || true)
  [[ -z "$hits" ]] && return 0
  status=1
  if [[ -n "${CI:-}" ]]; then
    cut -d: -f1 <<<"$hits" | sed "s|^|check-private: match in $label, line |" >&2
  else
    sed "s|^|$label:|" <<<"$hits" >&2
  fi
}

# Added lines of a diff, reported as file:line.
check_diff() {
  local file="" line=0 out=""
  while IFS= read -r l; do
    case "$l" in
      '+++ b/'*) file="${l#+++ b/}" ;;
      '+++ '*) file="" ;;
      '@@ '*)
        line=$(sed -E 's/^@@ -[0-9,]+ \+([0-9]+).*/\1/' <<<"$l")
        ;;
      '+'*)
        if [[ -n "$file" ]] && grep -q -i -E -- "$regex" <<<"${l#+}"; then
          status=1
          if [[ -n "${CI:-}" ]]; then
            out+="check-private: match in $file:$line"$'\n'
          else
            out+="$file:$line: ${l#+}"$'\n'
          fi
        fi
        line=$((line + 1))
        ;;
    esac
  done
  [[ -n "$out" ]] && printf '%s' "$out" >&2
  return 0
}

case "${1:-}" in
  --commit-msg)
    report "commit message" < <(grep -v '^#' "$2" || true)
    ;;
  --range)
    check_diff < <(git diff -U0 --no-color --no-ext-diff "$2")
    while read -r sha; do
      report "commit ${sha:0:12}" < <(git log -1 --format='%an <%ae>%n%cn <%ce>%n%B' "$sha")
    done < <(git rev-list "$2")
    ;;
  --stdin)
    report "${2:-input}"
    ;;
  "")
    check_diff < <(git diff --cached -U0 --no-color --no-ext-diff)
    report "commit identity" < <(printf '%s\n%s\n' "$(git var GIT_AUTHOR_IDENT)" "$(git var GIT_COMMITTER_IDENT)")
    ;;
  *)
    for f in "$@"; do report "$f" <"$f"; done
    ;;
esac

if [[ $status -ne 0 ]]; then
  echo "check-private: private data found (see your private patterns); blocked" >&2
fi
exit $status
