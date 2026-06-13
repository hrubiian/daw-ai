#!/usr/bin/env bash
# Fetch extra free, redistributable instruments & samples into the (gitignored)
# assets/ tree — the same "download locally, don't commit" model as GeneralUser.sf2.
#
# Everything here is CC0 or MIT, so it's safe to redistribute. Run from anywhere:
#   ./fetch-assets.sh
# Re-running skips anything already present.
set -euo pipefail
cd "$(dirname "$0")"
REPO="$(pwd)"

SF2_DIR="$REPO/assets"
SAMPLES_DIR="$REPO/assets/samples"
mkdir -p "$SF2_DIR" "$SAMPLES_DIR"

# Pick a downloader.
dl() { # dl <url> <dest>
  if command -v curl >/dev/null 2>&1; then curl -fL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then wget -O "$2" "$1"
  else echo "error: need curl or wget" >&2; exit 1; fi
}

echo "Fetching into $REPO/assets (gitignored) …"

# ── Soundfonts (.sf2 — rustysynth) ────────────────────────────────────────────
# NOTE: the bundled assets/GeneralUser.sf2 is actually "Fluid R3 GM" (Frank Wen,
# MIT) — verify with: head -c 2000 assets/GeneralUser.sf2 | strings | grep -i fluid
# So there's no point re-downloading FluidR3; you already have it as the default.
# For MORE GM/instrument soundfonts (license varies per file — check each):
#   https://musical-artifacts.com/artifacts?formats=sf2&license=free
#   https://www.polyphone.io/en/soundfonts
# Drop a .sf2 into assets/ and point a track's `soundfont` field at it
# (see assets/README.md). rustysynth loads .sf2 (use .sf2, not .sf3).
if [ -f "$SF2_DIR/GeneralUser.sf2" ]; then
  echo "  ✓ default soundfont present (assets/GeneralUser.sf2 = FluidR3 GM, MIT)"
else
  echo "  ! assets/GeneralUser.sf2 missing — grab a GM .sf2 (e.g. FluidR3, MIT):"
  echo "    https://github.com/fhunleth/midi_synth/releases/download/v0.1.0/FluidR3_GM.sf2"
fi

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
