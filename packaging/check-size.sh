#!/usr/bin/env bash
# Fail when an installer is over the download-size budget (architecture §13: < 40 MB).
#   packaging/check-size.sh FILE...        (budget in MB from SIZE_BUDGET_MB, default 40)
# Prints a table, and appends it to the GitHub job summary when run in Actions.
set -euo pipefail
budget_mb="${SIZE_BUDGET_MB:-40}"
budget=$((budget_mb * 1000 * 1000))
status=0
report="| Installer | Size (MB) | Budget (MB) | |\n| --- | ---: | ---: | --- |\n"
for f in "$@"; do
  bytes=$(wc -c < "$f")
  mb=$(awk -v b="$bytes" 'BEGIN { printf "%.1f", b / 1000000 }')
  if [ "$bytes" -gt "$budget" ]; then
    mark="over"; status=1
    echo "::error title=Size budget::$(basename "$f") is ${mb} MB, over the ${budget_mb} MB budget" >&2
  else
    mark="ok"
  fi
  report+="| $(basename "$f") | $mb | $budget_mb | $mark |\n"
done
printf "%b" "$report"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then printf "%b" "$report" >> "$GITHUB_STEP_SUMMARY"; fi
exit "$status"
