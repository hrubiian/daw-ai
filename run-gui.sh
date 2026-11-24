#!/usr/bin/env bash
# Launch the GPUI desktop frontend (`--gui`) under XWayland.
#
# Compatibility launcher for systems that need XWayland. The GUI now draws its
# own titlebar and resize handles on native Wayland as well. XMODIFIERS=@im=none
# avoids an XIM input-method panic seen on this XWayland setup.
#
# Any extra args are passed through, e.g.:  ./run-gui.sh my-song.daw.json
# Use --cached to start the last built executable if one exists.
repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)" || exit
launch=(cargo run --manifest-path "$repo/Cargo.toml" --)
if [[ ${1:-} == --cached ]]; then
  shift
  target_dir="${CARGO_TARGET_DIR:-${CARGO_BUILD_TARGET_DIR:-"$repo/target"}}"
  binary="$target_dir/debug/daw-ai"
  if [[ -f "$binary" && -x "$binary" ]]; then
    launch=("$binary")
  fi
fi

exec env -u WAYLAND_DISPLAY \
  DISPLAY="${DISPLAY:-:0}" \
  XMODIFIERS=@im=none GTK_IM_MODULE=none QT_IM_MODULE=none \
  "${launch[@]}" --gui "$@"
