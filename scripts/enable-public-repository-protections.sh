#!/usr/bin/env bash
# Run after the owner explicitly publishes the repository, or upgrades its plan.
set -euo pipefail
repo=nommyt/airbend
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
gh api --method PUT "repos/$repo/branches/main/protection" \
  --input "$script_dir/../.github/branch-protection.json"
if [[ $(gh repo view "$repo" --json isPrivate --jq .isPrivate) == false ]]; then
  gh api --method PUT "repos/$repo/private-vulnerability-reporting"
  gh api --method PATCH "repos/$repo" --input - <<'JSON'
{"security_and_analysis":{"secret_scanning":{"status":"enabled"},"secret_scanning_push_protection":{"status":"enabled"}}}
JSON
fi
