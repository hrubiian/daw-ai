#!/usr/bin/env bash
# Launch the GPUI desktop frontend (`--gui`) under XWayland.
#
# Compatibility launcher for systems that need XWayland. The GUI now draws its
# own titlebar and resize handles on native Wayland as well. XMODIFIERS=@im=none
# avoids an XIM input-method panic seen on this XWayland setup.
#
# Any extra args are passed through, e.g.:  ./run-gui.sh my-song.daw.json
exec env -u WAYLAND_DISPLAY \
  DISPLAY="${DISPLAY:-:0}" \
  XMODIFIERS=@im=none GTK_IM_MODULE=none QT_IM_MODULE=none \
  cargo run -- --gui "$@"
