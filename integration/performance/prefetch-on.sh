#!/usr/bin/env bash
set -euo pipefail
export AXTON_EXPERIMENTAL_BOOTSTRAP_PREFETCH=1
exec bash "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/run.sh" "$@"
