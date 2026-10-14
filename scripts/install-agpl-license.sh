#!/usr/bin/env bash
set -euo pipefail
# Fetch canonical full text from the GNU project before the first public release.
curl --fail --location --silent --show-error --proto '=https' --tlsv1.2   'https://www.gnu.org/licenses/agpl-3.0.txt' -o LICENSE
if ! grep -q '13. Remote Network Interaction' LICENSE; then
  echo 'Unexpected license text; verify download and do not commit.' >&2
  exit 1
fi
echo 'AGPLv3 text saved to LICENSE. Review and commit it.'
