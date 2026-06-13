#!/usr/bin/env bash
# Launch the GPUI desktop frontend (`--gui`) under XWayland.
#
# Why: with gpui 0.2.2 on GNOME/Wayland the window is borderless and not
# resizable (gpui requests server-side decorations, which GNOME doesn't provide
# for Wayland-native apps). Running under XWayland makes mutter draw a normal
# decorated, edge-resizable window. XMODIFIERS=@im=none avoids an XIM
# input-method panic seen on this XWayland setup.
#
# Any extra args are passed through, e.g.:  ./run-gui.sh my-song.daw.json
exec env -u WAYLAND_DISPLAY \
  DISPLAY="${DISPLAY:-:0}" \
  XMODIFIERS=@im=none GTK_IM_MODULE=none QT_IM_MODULE=none \
  cargo run -- --gui "$@"
