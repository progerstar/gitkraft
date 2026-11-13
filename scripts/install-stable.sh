#!/usr/bin/env sh
# Run install-local.sh with the stable Rust toolchain, whatever the default
# toolchain is (a nightly or esp toolchain may be too old for the deps).
# Arguments are passed on, e.g. --uninstall.
set -eu

RUSTUP_TOOLCHAIN=stable exec "$(dirname "$0")/install-local.sh" "$@"
