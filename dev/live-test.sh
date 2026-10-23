#!/usr/bin/env bash
# Run the IMAP live tests against a real (throwaway!) account whose settings are in a
# git-ignored env file, e.g. .env.gmail-test. The file is loaded, never printed.
#
#   dev/live-test.sh .env.gmail-test [test name filter]
#
# The tests EMPTY the account's INBOX and Trash; the file must set TERN_LIVE_WIPE_OK to the
# same address as TERN_IMAP_USER to confirm that's fine.
set -euo pipefail

file="${1:?usage: $0 ENV_FILE [filter]}"
shift
[[ -f "$file" ]] || { echo "no such file: $file" >&2; exit 1; }
if git -C "$(dirname "$file")" ls-files --error-unmatch "$(basename "$file")" >/dev/null 2>&1; then
    echo "$file is tracked by git; secrets must stay in an ignored file" >&2
    exit 1
fi
set -a
# shellcheck disable=SC1090
source "$file"
set +a
for v in TERN_IMAP_HOST TERN_IMAP_USER TERN_IMAP_PASS TERN_LIVE_WIPE_OK; do
    [[ -n "${!v:-}" ]] || { echo "$v is empty in $file" >&2; exit 1; }
done
echo "live tests against $TERN_IMAP_USER at $TERN_IMAP_HOST"
cd "$(dirname "$0")/.."
exec cargo test -p tern-imap --test live -- --ignored --nocapture --test-threads=1 "$@"
