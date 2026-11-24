#!/usr/bin/env bash
# Set up the default SoundFont and optionally fetch the VCSL sample library.
# Run from anywhere. Existing assets remain in place.
set -euo pipefail
cd "$(dirname "$0")"
REPO="$(pwd)"

if (( $# > 1 )) || [[ -n "${1:-}" && "$1" != --quickstart ]]; then
  echo "Usage: $0 [--quickstart]" >&2
  exit 2
fi

SF2_DIR="$REPO/assets"
SAMPLES_DIR="$REPO/assets/samples"
default_soundfont="$SF2_DIR/GeneralUser.sf2"
system_soundfont=/usr/share/sounds/sf2/FluidR3_GM.sf2
if [[ -f "$default_soundfont" && -s "$default_soundfont" ]]; then
  echo "Using existing $default_soundfont"
elif [[ -e "$default_soundfont" || -L "$default_soundfont" ]]; then
  echo "Repair or replace $default_soundfont before running setup." >&2
  exit 1
elif [[ -f "$system_soundfont" && -s "$system_soundfont" ]]; then
  mkdir -p "$SF2_DIR"
  ln -sT -- "$system_soundfont" "$default_soundfont"
  echo "Linked $default_soundfont to $system_soundfont"
else
  echo "Default SoundFont missing. Install fluid-soundfont-gm (Debian/Ubuntu) and retry," >&2
  echo "or place a General MIDI .sf2 file at $default_soundfont." >&2
  exit 1
fi
if [[ "${1:-}" == --quickstart ]]; then exit 0; fi

mkdir -p "$SAMPLES_DIR"

# ── Sample library (WAV — Sampler pads / audio clips) ─────────────────────────
# VCSL — Versilian Community Sample Library, CC0. A broad multi-instrument WAV
# set (drums, percussion, keys, orchestral). Shallow clone to keep it lean.
VCSL="$SAMPLES_DIR/vcsl"
if [ -d "$VCSL/.git" ] || [ -d "$VCSL/VCSL" ]; then
  echo "  ✓ VCSL already present ($VCSL)"
elif command -v git >/dev/null 2>&1; then
  echo "  ↓ VCSL (CC0) via git …"
  git clone --depth 1 https://github.com/sgossner/VCSL.git "$VCSL"
else
  echo "  ! git not found — skipping VCSL. Install git, or download the zip from"
  echo "    https://github.com/sgossner/VCSL and unzip into $VCSL"
fi

cat <<EOF

Done. See assets/README.md for licenses and how to wire these into a project
(Sampler pads, per-track soundfont, audio clips).

More browsing (verify each item's license before use):
  • Soundfonts: https://musical-artifacts.com/artifacts?formats=sf2&license=free
                https://www.polyphone.io/en/soundfonts
  • CC0 samples: https://freesound.org/browse/tags/cc0/
EOF
