#!/usr/bin/env bash
# Fails if staged changes (or the files given as arguments) contain any pattern
# from the local, gitignored file `.private-patterns` (one extended regex per
# line, '#' comments allowed). Keeps personal data (real names, emails,
# domains, hosts, IPs, local paths) out of the repository.
# Install as a hook:  ln -s ../../scripts/check-private.sh .git/hooks/pre-commit
set -euo pipefail
root=$(git rev-parse --show-toplevel)
patterns="${PRIVATE_PATTERNS_FILE:-$root/.private-patterns}"
if [[ ! -f "$patterns" ]]; then
  echo "check-private: no $patterns file, skipping" >&2
  exit 0
fi
regex=$(grep -v -e '^\s*#' -e '^\s*$' "$patterns" | paste -sd'|' -)
[[ -z "$regex" ]] && exit 0
status=0
if [[ $# -gt 0 ]]; then
  grep -n -i -E "$regex" -- "$@" && status=1
else
  git diff --cached -U0 --no-color | grep -E '^\+' | grep -i -E "$regex" && status=1
  # the commit identity too
  ident="$(git var GIT_AUTHOR_IDENT) $(git var GIT_COMMITTER_IDENT)"
  echo "$ident" | grep -i -E "$regex" && { echo "commit identity matches a private pattern"; status=1; }
fi
if [[ $status -ne 0 ]]; then
  echo "check-private: private data found (see .private-patterns); commit blocked" >&2
fi
exit $status
