#!/usr/bin/env bash
# Tern's audit: known vulnerabilities, licenses and sources, unused dependencies, lints.
#
#   dev/audit.sh          # everything; exit status 1 if any check fails
#
# Needs cargo-audit, cargo-deny and cargo-machete (`cargo install --locked <name>`).
# Unused *public* code across crates isn't covered here (the compiler only flags private
# code); ask GitNexus for functions nothing calls.
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

failed=()
run() {
    local name=$1
    shift
    printf '\n== %s\n' "$name"
    if ! command -v "$1" >/dev/null && ! cargo "$2" --version >/dev/null 2>&1; then
        echo "skipped: $1 not installed"
        return
    fi
    "$@" || failed+=("$name")
}

run "RustSec advisories (cargo audit)" cargo audit
run "Licenses, bans, sources, advisories (cargo deny)" cargo deny --log-level error check
run "Unused dependencies (cargo machete)" cargo machete
run "Lints (clippy, warnings are errors)" cargo clippy -q --workspace --all-targets -- -D warnings

echo
if ((${#failed[@]})); then
    printf 'FAILED: %s\n' "${failed[@]}"
    exit 1
fi
echo "All checks passed."
