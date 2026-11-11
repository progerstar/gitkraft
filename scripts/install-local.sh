#!/usr/bin/env sh
# Build the GUI in release mode and install it for the current user:
#   binary   → ~/.local/bin/gitkraft
#   launcher → ~/.local/share/applications/
#   icons    → ~/.local/share/icons/hicolor/
# No root needed. Run again after `git pull` to update.
# Uninstall: scripts/install-local.sh --uninstall
set -eu

APP_ID=io.github.sorinirimies.GitKraft
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN_DIR="$HOME/.local/bin"
APPS_DIR="$HOME/.local/share/applications"
ICONS_DIR="$HOME/.local/share/icons/hicolor"
SIZES="16 22 24 32 48 64 96 128 256 512"

refresh_caches() {
    command -v update-desktop-database >/dev/null && update-desktop-database "$APPS_DIR" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$ICONS_DIR" || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN_DIR/gitkraft" "$APPS_DIR/$APP_ID.desktop" "$ICONS_DIR/scalable/apps/$APP_ID.svg"
    for n in $SIZES; do rm -f "$ICONS_DIR/${n}x${n}/apps/$APP_ID.png"; done
    refresh_caches
    echo "GitKraft removed (settings in ~/.config/gitkraft are kept)."
    exit 0
fi

cd "$ROOT"
cargo build --release --locked -p gitkraft

install -Dm755 target/release/gitkraft "$BIN_DIR/gitkraft"

# Absolute Exec path: launchers do not always have ~/.local/bin in PATH.
mkdir -p "$APPS_DIR"
sed "s|^Exec=gitkraft|Exec=$BIN_DIR/gitkraft|" \
    "packaging/linux/$APP_ID.desktop" > "$APPS_DIR/$APP_ID.desktop"

install -Dm644 assets/icons/gitkraft.svg "$ICONS_DIR/scalable/apps/$APP_ID.svg"
for n in $SIZES; do
    install -Dm644 "assets/icons/gitkraft-$n.png" "$ICONS_DIR/${n}x${n}/apps/$APP_ID.png"
done

refresh_caches
echo "Installed: $BIN_DIR/gitkraft (launcher: GitKraft)"
