#!/usr/bin/env bash
# Generates the Node and React Native clients and the backend types from one schema.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$root/scripts/env.sh"
cd "$root"
cargo run -p axton-compiler --locked -- compile examples/todo/models examples/todo/generated/node \
  --backend-runtime ../../../../packages/server/index.mts \
  --client-runtime ../../../../packages/client-js/index.mts
cargo run -p axton-compiler --locked -- compile examples/todo/models examples/todo/generated/mobile \
  --backend-runtime ../../../../packages/server/index.mts \
  --client-runtime ../../../../packages/client-react-native/index.ts
