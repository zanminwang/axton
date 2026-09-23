#!/usr/bin/env bash
# Uses only disposable test databases and simulator installs selected for this harness.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$root/scripts/env.sh"
app_dir="$root/integration/platform/react-native"
bundle_id="dev.axton.ReactNativeSmoke"
app_bundle="${AXTON_RN_APP_BUNDLE:-$app_dir/ios/build/Build/Products/Release-iphonesimulator/axtonrnharness.app}"
run_dir="$(mktemp -d "${TMPDIR:-/tmp}/axton-rn-smoke.XXXXXX")"
backend_pid=""
created_devices=()
alice="${1:-}"
bob="${2:-}"
cleanup() {
  if [[ -n "$backend_pid" ]]; then kill "$backend_pid" 2>/dev/null || true; wait "$backend_pid" 2>/dev/null || true; fi
  pg_ctl -D "$run_dir/pg" -m immediate stop >/dev/null 2>&1 || true
  rm -rf "$run_dir/pg"
  for device in ${created_devices[@]+"${created_devices[@]}"}; do
    xcrun simctl shutdown "$device" >/dev/null 2>&1 || true
    xcrun simctl delete "$device" >/dev/null 2>&1 || true
  done
  echo "Runtime evidence: $run_dir"
}
trap cleanup EXIT
if [[ ! -d "$app_bundle" ]]; then echo "Build the Release simulator app first; set AXTON_RN_APP_BUNDLE to its .app path." >&2; exit 1; fi
if [[ -z "$alice" && -z "$bob" ]]; then
  # Default to the newest installed iOS runtime and the first iPhone device type it supports.
  runtime="${AXTON_RN_SIM_RUNTIME:-$(xcrun simctl list runtimes -j | python3 -c 'import json,sys;r=[x for x in json.load(sys.stdin)["runtimes"] if x["platform"]=="iOS" and x["isAvailable"]];r.sort(key=lambda x:[int(p) for p in x["version"].split(".")]);print(r[-1]["identifier"])')}"
  kind="${AXTON_RN_SIM_DEVICE:-$(xcrun simctl list devicetypes -j | python3 -c 'import json,sys;print(next(x["identifier"] for x in json.load(sys.stdin)["devicetypes"] if x["identifier"].startswith("com.apple.CoreSimulator.SimDeviceType.iPhone-")))')}"
  alice="$(xcrun simctl create "AXTON RN Alice $$" "$kind" "$runtime")";created_devices+=("$alice")
  bob="$(xcrun simctl create "AXTON RN Bob $$" "$kind" "$runtime")";created_devices+=("$bob")
fi
[[ -n "$alice" && -n "$bob" && "$alice" != "$bob" ]] || { echo 'Supply two different simulator UDIDs, or neither.' >&2;exit 1; }
# Only a fresh harness install is accepted; never erase existing caller data.
for device in "$alice" "$bob"; do
  if xcrun simctl get_app_container "$device" "$bundle_id" data >/dev/null 2>&1; then echo "Harness already installed on $device; use fresh test simulators." >&2;exit 1;fi
  xcrun simctl boot "$device" >/dev/null 2>&1 || true
  xcrun simctl bootstatus "$device" -b
  xcrun simctl install "$device" "$app_bundle"
done
alice_data="$(xcrun simctl get_app_container "$alice" "$bundle_id" data)/Documents"
bob_data="$(xcrun simctl get_app_container "$bob" "$bundle_id" data)/Documents"
mkdir -p "$alice_data" "$bob_data"
port="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
initdb -D "$run_dir/pg" -A trust --no-locale -E UTF8 >/dev/null
pg_ctl -D "$run_dir/pg" -l "$run_dir/pg.log" -o "-p $port -h 127.0.0.1 -k $run_dir" start >/dev/null
export DATABASE_URL="postgresql://$(id -un)@127.0.0.1:$port/postgres"
export AXTON_SMOKE_PORTS="$run_dir/ports.json"
node "$app_dir/server.mts" >"$run_dir/backend.log" 2>&1 & backend_pid="$!"
for _ in $(seq 1 60); do [[ -f "$AXTON_SMOKE_PORTS" ]] && break;sleep 1;done
[[ -f "$AXTON_SMOKE_PORTS" ]] || { cat "$run_dir/backend.log";exit 1; }
control="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["control"])' "$AXTON_SMOKE_PORTS")"
launch_phase() {
  local device="$1" data="$2" user="$3" phase="$4"
  xcrun simctl terminate "$device" "$bundle_id" >/dev/null 2>&1 || true
  python3 - "$data" "$user" "$phase" "$AXTON_SMOKE_PORTS" <<'PY'
import json,sys
from pathlib import Path
folder,user,phase,ports=sys.argv[1:]
p=Path(folder)
config={'user':user,'phase':phase,'url':json.load(open(ports))[user]}
result=p/'result.json'
if result.exists():
 old=json.loads(result.read_text())
 if old.get('clientId'):config['expectedClientId']=old['clientId']
 result.unlink()
(p/'config.json').write_text(json.dumps(config))
PY
  xcrun simctl launch "$device" "$bundle_id"
}
wait_phase() {
  local device="$1" data="$2" user="$3" phase="$4"
  for _ in $(seq 1 150); do
    if [[ -f "$data/result.json" ]]; then
      python3 - "$data/result.json" "$phase" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]));assert r.get('ok') and r.get('phase')==sys.argv[2],r
print(json.dumps(r))
PY
      cp "$data/result.json" "$run_dir/$user-$phase.json"
      xcrun simctl io "$device" screenshot "$run_dir/$user-$phase.png" >/dev/null
      return
    fi
    sleep 1
  done
  echo "Timeout: $user $phase" >&2; cat "$run_dir/backend.log";return 1
}
launch_phase "$alice" "$alice_data" alice online
launch_phase "$bob" "$bob_data" bob online
wait_phase "$alice" "$alice_data" alice online
wait_phase "$bob" "$bob_data" bob online
curl --fail --silent -X POST "$control/alice/offline" >"$run_dir/disconnected.json"
launch_phase "$alice" "$alice_data" alice offline
wait_phase "$alice" "$alice_data" alice offline
launch_phase "$alice" "$alice_data" alice restart
wait_phase "$alice" "$alice_data" alice restart
launch_phase "$bob" "$bob_data" bob remote
wait_phase "$bob" "$bob_data" bob remote
curl --fail --silent -X POST "$control/alice/online" >"$run_dir/reconnected.json"
launch_phase "$alice" "$alice_data" alice settle
wait_phase "$alice" "$alice_data" alice settle
launch_phase "$bob" "$bob_data" bob observe
wait_phase "$bob" "$bob_data" bob observe
curl --fail --silent "$control" >"$run_dir/server-result.json"
python3 - "$run_dir" <<'PY'
import json,sys
from pathlib import Path
p=Path(sys.argv[1]);read=lambda name:json.loads((p/name).read_text())
a,b,server=read('alice-settle.json'),read('bob-observe.json'),read('server-result.json')
assert a['clientId']!=b['clientId'], 'shared identity'
normalize=lambda rows:sorted(rows,key=lambda r:r['id'])
assert normalize(a['rows'])==normalize(b['rows'])==normalize(server['rows']), (a,b,server)
assert a['pending']==b['pending']==0
assert server['calls']=={'add':1,'edit':4},server
assert server['droppedResponses']==1,server
assert len(server['rows'])==2,server
print('PASS: two real RN clients, lost-response retry, offline writes, process restart and convergence')
PY
