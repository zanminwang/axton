#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
source "$root/scripts/env.sh"
if [[ -x "$root/.tools/cargo/bin/cargo" ]]; then
  export CARGO_HOME="$root/.tools/cargo" RUSTUP_HOME="$root/.tools/rustup"
  export PATH="$CARGO_HOME/bin:$PATH"
fi
node "$root/bindings/node/build.mjs" --probe
cd "$root/integration/bindings/node"
npm ci
npm run generate
# Tests deliberately write probe tables. Always create a disposable local cluster.
probe_dir="$(mktemp -d "${TMPDIR:-/tmp}/axton-node-pg.XXXXXX")"
cleanup() {
  pg_ctl -D "$probe_dir/data" -m immediate stop >/dev/null 2>&1 || true
  rm -rf -- "$probe_dir"
}
trap cleanup EXIT
probe_port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
initdb -D "$probe_dir/data" -A trust --no-locale -E UTF8 >/dev/null
pg_ctl -D "$probe_dir/data" -l "$probe_dir/postgres.log" -o "-p $probe_port -h 127.0.0.1 -k $probe_dir" start >/dev/null
export DATABASE_URL="postgresql://$(id -un)@127.0.0.1:$probe_port/postgres"
node --test transaction-bridge.test.mjs
