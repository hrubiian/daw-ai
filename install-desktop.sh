#!/usr/bin/env bash
# Install a GNOME/freedesktop launcher + icon for daw-ai, so it shows up in
# Activities and can be pinned to the dash/taskbar. User-level (no sudo).
#
# It builds the release binary, installs the icon at the standard hicolor sizes,
# and writes ~/.local/share/applications/daw-ai.desktop with the absolute binary
# path. The launcher forces XWayland for a normal decorated window (like run-gui.sh),
# and the window's app_id/WM_CLASS ("daw-ai") matches StartupWMClass so the running
# window associates with the launcher for pinning.
set -euo pipefail
cd "$(dirname "$0")"
REPO="$(pwd)"

echo "Building release binary…"
cargo build --release --locked
BIN="$REPO/target/release/daw-ai"
[ -x "$BIN" ] || { echo "error: build did not produce $BIN" >&2; exit 1; }

ICON_DIR="$HOME/.local/share/icons/hicolor"
APP_DIR="$HOME/.local/share/applications"
mkdir -p "$APP_DIR"

echo "Installing icons…"
for sz in 256 128 64 48 32; do
  install -Dm644 "assets/icons/daw-ai-${sz}.png" "$ICON_DIR/${sz}x${sz}/apps/daw-ai.png"
done
install -Dm644 "assets/icons/daw-ai.svg" "$ICON_DIR/scalable/apps/daw-ai.svg"

echo "Installing launcher…"
sed -e "s|__DAW_AI_BIN__|$BIN|g" -e "s|__DAW_AI_DIR__|$REPO|g" \
  packaging/daw-ai.desktop.in > "$APP_DIR/daw-ai.desktop"
chmod 644 "$APP_DIR/daw-ai.desktop"

# Refresh caches (best-effort; harmless if the tools are absent).
update-desktop-database "$APP_DIR" 2>/dev/null || true
gtk-update-icon-cache -f "$ICON_DIR" 2>/dev/null || true

cat <<EOF

Installed:
  $APP_DIR/daw-ai.desktop
  $ICON_DIR/<size>/apps/daw-ai.png  (+ scalable/daw-ai.svg)

Next: open Activities → search "daw-ai" → launch it, then right-click its dash
icon → "Pin to Dash". If it doesn't appear right away, log out and back in.

To uninstall: rm "$APP_DIR/daw-ai.desktop" and the daw-ai icons under "$ICON_DIR".
EOF
