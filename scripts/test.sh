#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$root/scripts/env.sh"
cd "$root"
npm ci
bash scripts/build.sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
(cd integration/e2e/fixtures/round-trip && npm ci && npx prisma generate)
cargo run -p axton-compiler -- compile integration/e2e/fixtures/round-trip/models integration/e2e/fixtures/round-trip/generated --backend-runtime ../../../../../packages/server/index.mts --client-runtime ../../../../../packages/client-js/index.mts
(cd examples/todo && npm ci && npx prisma generate)
bash examples/todo/generate.sh
npm run typecheck
"$root/node_modules/.bin/prettier" --check packages/client-js/*.mts packages/server/*.mts packages/postgres/*.mts packages/postgres/src/*.mts packages/client-react-native/*.mts packages/client-react-native/index.ts
"$root/node_modules/.bin/tsc" -p packages/client-react-native
node --test packages/client-react-native/plugins/expo-path-spaces.test.cjs
node --test integration/bindings/client-js/*.test.mjs
node --test integration/bindings/client-react-native/*.test.mjs
bash integration/persistence/transaction-probe/run.sh
bash integration/persistence/server/run.sh
case "$(uname -s)" in
 Darwin) export AXTON_LIBRARY="$root/target/debug/libaxton_dart.dylib";;
 Linux) export AXTON_LIBRARY="$root/target/debug/libaxton_dart.so";;
 *) echo 'Use the documented platform-specific native library path on this host.' >&2; exit 1;;
esac
export AXTON_DART_LIBRARY="$AXTON_LIBRARY"
(cd packages/dart && dart pub get && dart analyze && dart test)
bash integration/generated-api/verify.sh
bash integration/e2e/run.sh
node --test integration/e2e/todo-ui.test.mjs
bash integration/e2e/todo-run.sh
python3 website/scripts/check_examples.py
