#!/usr/bin/env bash
set -euo pipefail
root="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
source "$root/scripts/env.sh"
cd "$root"
export AXTON_DART="${AXTON_DART:-$(command -v dart)}"
case "$(uname -s)" in
  Darwin) export AXTON_DART_LIBRARY="$root/target/release/libaxton_dart.dylib" ;;
  Linux) export AXTON_DART_LIBRARY="$root/target/release/libaxton_dart.so" ;;
  *) echo 'Supported hosts: macOS and Linux' >&2; exit 1 ;;
esac
mkdir -p artifacts/performance/control
control=294f3563dc42ce5c0d875cbf141d425b65399f52
for file in persistence.mts sql.mts; do
  git show "$control:packages/backend/postgres/src/$file" > "artifacts/performance/control/$file"
done
cp integration/performance/baselines/2026-10-28-mutation-roundtrips/runner.mjs artifacts/performance/roundtrip-runner.mjs
export AXTON_EXPERIMENTAL_BOOTSTRAP_PREFETCH=0
if [[ "${AXTON_PERFORMANCE_SKIP_BUILD:-0}" != 1 ]]; then
  [[ -d node_modules ]] || npm ci
  cargo build --release --locked -p axton-dart -p axton-compiler
  node bindings/node/build.mjs --release
  node_modules/.bin/tsc -b scripts/release/tsconfig.json
  target/release/axton compile integration/performance/fixture/schema integration/performance/fixture/generated \
    --backend-runtime ../../../../packages/backend/server/index.mts \
    --client-runtime ../../../../packages/frontend/client-js/index.mts
  "$AXTON_DART" pub get --directory integration/performance
  "$AXTON_DART" analyze integration/performance
  "$AXTON_DART" compile exe integration/performance/initial-sync/client.dart -o artifacts/performance/initial-sync
  "$AXTON_DART" compile exe integration/performance/live-sync/client.dart -o artifacts/performance/live-sync
  node integration/performance/run.mjs --record-build
fi
cluster="$(mktemp -d "${TMPDIR:-/tmp}/axton-performance-pg.XXXXXX")"
# The shell owns client files too, including its forced-runner-stop fallback.
export AXTON_PERFORMANCE_TEMP="$cluster"
runner=""
cleanup() {
  if [[ -n "$runner" ]]; then
    kill -TERM "$runner" 2>/dev/null || true
    for _ in {1..50}; do
      kill -0 "$runner" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL "$runner" 2>/dev/null || true
    wait "$runner" 2>/dev/null || true
  fi
  pg_ctl -D "$cluster/data" -m immediate stop >/dev/null 2>&1 || true
  rm -rf -- "$cluster"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
port="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
if ! initdb -D "$cluster/data" -A trust --no-locale -E UTF8 >"$cluster/init.log" 2>&1; then
  cat "$cluster/init.log" >&2
  node -e 'const fs=require("node:fs");fs.writeFileSync("artifacts/performance/setup-failure.json",JSON.stringify({status:"setup-failed",stage:"initdb",error:fs.readFileSync(process.argv[1],"utf8"),at:new Date().toISOString()},null,2)+"\n")' "$cluster/init.log"
  exit 1
fi
pg_ctl -D "$cluster/data" -l "$cluster/postgres.log" -o "-p $port -h 127.0.0.1 -k $cluster" start >/dev/null
DATABASE_URL="postgresql://$(id -un)@127.0.0.1:$port/postgres"
export DATABASE_URL
node artifacts/performance/roundtrip-runner.mjs "$@" &
runner=$!
wait "$runner"
runner=""
