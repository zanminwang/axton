#!/usr/bin/env bash
# Builds the runtime, regenerates the To-do backend and clients, starts a disposable
# PostgreSQL cluster, and serves the demo backend (PORT, default 4242).
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$root/scripts/env.sh"
bash "$root/scripts/build.sh"
bash "$root/examples/todo/generate.sh"
(cd "$root/examples/todo" && npm ci && npm run generate)
cluster="$(mktemp -d "${TMPDIR:-/tmp}/axton-todo-pg.XXXXXX")"
cleanup(){ pg_ctl -D "$cluster/data" -m immediate stop >/dev/null 2>&1 || true; rm -rf -- "$cluster"; }
trap cleanup EXIT
port="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
initdb -D "$cluster/data" -A trust --no-locale -E UTF8 >/dev/null
pg_ctl -D "$cluster/data" -l "$cluster/log" -o "-p $port -h 127.0.0.1 -k $cluster" start >/dev/null
export DATABASE_URL="postgresql://$(id -un)@127.0.0.1:$port/postgres"
node "$root/examples/todo/server.mts"
