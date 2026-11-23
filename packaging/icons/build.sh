#!/usr/bin/env bash
# Regenerate every packaged app icon from the brand generator (brand/src/icon_d.py).
# Needs: python3 with Pillow, numpy, scipy, matplotlib; rsvg-convert (apt: librsvg2-bin).
# The output is committed, so packaging never needs Python. Re-run after changing the mark.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
python3 "$root/brand/src/export_icons.py" "$here"
echo "icons written to $here"
