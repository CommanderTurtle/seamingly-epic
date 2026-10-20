#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
release="$root/target/release"
binary="$release/seamingly-epic"

if [[ ! -f "$binary" || ! -x "$binary" ]]; then
  printf 'Release binary not found or not executable: %s\nRun: cargo build --release\n' "$binary" >&2
  exit 1
fi

target_user="${SUDO_USER:-${USER:-$(id -un)}}"
if command -v getent >/dev/null 2>&1; then
  target_home="$(getent passwd "$target_user" | cut -d: -f6 || true)"
else
  target_home="${HOME:?HOME is not set and getent is unavailable}"
fi
if [[ -z "$target_home" || ! -d "$target_home" ]]; then
  printf 'Could not resolve a home directory for %s.\n' "$target_user" >&2
  exit 1
fi

bashrc="$target_home/.bashrc"
path_line="export PATH=\"$release:\$PATH\""
touch "$bashrc"
if ! grep -Fqx -- "$path_line" "$bashrc"; then
  printf '\n# Seamingly Epic commands\n%s\n' "$path_line" >> "$bashrc"
  printf 'Added to %s: %s\n' "$bashrc" "$release"
else
  printf 'Bash PATH already contains the Seamingly Epic release entry in %s.\n' "$bashrc"
fi

if [[ $(id -u) -eq 0 && "$target_user" != root ]]; then
  chown "$target_user":"$(id -gn "$target_user")" "$bashrc"
fi

export PATH="$release:$PATH"
command -v seamingly-epic >/dev/null

printf 'seamingly-epic: %s\n' "$(command -v seamingly-epic)"
printf 'This script refreshed PATH for itself. Run: source %q\n' "$bashrc"
printf 'New shells and subprocesses launched from them inherit the command.\n'
