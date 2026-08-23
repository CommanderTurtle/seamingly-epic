#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if ! command -v cargo >/dev/null 2>&1; then
  echo 'Rust/Cargo is required. Install Rust from https://rustup.rs and run this script again.' >&2
  exit 1
fi

cd "$project_root"
cargo build --release --locked
install -Dm755 \
  "$project_root/target/release/seamingly-epic" \
  "$project_root/bin/seamingly-epic"
echo 'Seamingly Epic is ready. Restart ComfyUI to load the custom nodes.'
