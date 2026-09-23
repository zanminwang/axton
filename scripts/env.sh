#!/usr/bin/env bash
# Prefer the optional workspace-local Rust install; otherwise use the developer's toolchain.
axton_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -x "$axton_root/.tools/cargo/bin/cargo" ]]; then
 export CARGO_HOME="$axton_root/.tools/cargo"
 export RUSTUP_HOME="$axton_root/.tools/rustup"
 export PATH="$axton_root/.tools/cargo/bin:$PATH"
fi
