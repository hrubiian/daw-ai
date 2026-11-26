#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

mkdir -p assets/sfz assets/soundfonts

convert_to_wav() {
  local dir="$1"
  command -v ffmpeg >/dev/null 2>&1 || { echo "ffmpeg not found; skipping WAV conversion in $dir"; return 0; }
  find "$dir" -type f \( -iname '*.flac' -o -iname '*.ogg' \) -print0 | \
    xargs -0 -r -P 4 -I {} bash -c '
      src="$1"
      dst="${src%.*}.wav"
      if [ ! -f "$dst" ]; then
        ffmpeg -y -loglevel error -i "$src" "$dst" && rm -f "$src"
      fi
    ' _ {}
}

clone_shallow() {
  local url="$1"
  local dst="$2"
  if [ -d "$dst" ] && [ -n "$(ls -A "$dst" 2>/dev/null)" ]; then
    echo "already present: $dst"
    return 0
  fi
  rm -rf "$dst"
  git clone --depth 1 "$url" "$dst"
  rm -rf "$dst/.git"
}

echo "=== 1. AVL Drumkits (Black Pearl 5pc / Red Zeppelin) ==="
clone_shallow https://github.com/studiorack/avl-drumkits.git assets/sfz/avl-drumkits
convert_to_wav assets/sfz/avl-drumkits

echo "=== 2. Karoryfer Meatbass (1958 Otto Rubner Upright Bass) ==="
clone_shallow https://github.com/sfzinstruments/karoryfer.meatbass.git assets/sfz/meatbass
convert_to_wav assets/sfz/meatbass

echo "=== 3. Karoryfer Weresax (Alto Sax) ==="
clone_shallow https://github.com/sfzinstruments/karoryfer.weresax.git assets/sfz/weresax
convert_to_wav assets/sfz/weresax

echo "=== 4. Karoryfer Bear Sax (1926 Conn Baritone Sax) ==="
clone_shallow https://github.com/sfzinstruments/karoryfer.bear-sax.git assets/sfz/bear-sax
convert_to_wav assets/sfz/bear-sax

echo "=== 5. VSCO-2-CE Brass + Upright Piano (SFZ mappings + needed master WAVs) ==="
rm -rf assets/sfz/vsco2-wav-tmp
if [ ! -f assets/sfz/vsco2/TrumpetSus.sfz ]; then
  rm -rf assets/sfz/vsco2
  git clone --branch SFZ --depth 1 https://github.com/sgossner/VSCO-2-CE.git assets/sfz/vsco2
  rm -rf assets/sfz/vsco2/.git
fi
python3 - <<'PY'
import concurrent.futures, os, re, urllib.parse, urllib.request

BASE = "assets/sfz/vsco2"
SFZ_FILES = [
    "TrumpetSus.sfz",
    "TrumpetSusVib.sfz",
    "TrumpetHarmonMuteSus.sfz",
    "TromboneSus.sfz",
    "TromboneStac.sfz",
    "TromboneVib.sfz",
    "UprightPiano.sfz",
]
needed = []
for sfz in SFZ_FILES:
    txt = open(os.path.join(BASE, sfz), errors="replace").read()
    dp_m = re.search(r'default_path=([^\r\n]+)', txt)
    dp = dp_m.group(1).strip().replace("\\", "/") if dp_m else ""
    for smp in re.findall(r'sample=([^\r\n]+)', txt):
        rel = (dp + smp.strip().replace("\\", "/")).lstrip("/")
        if rel not in needed:
            needed.append(rel)

print(f"Downloading {len(needed)} VSCO-2-CE WAV samples...")

def download_one(rel):
    dst = os.path.join(BASE, rel)
    if os.path.exists(dst) and os.path.getsize(dst) > 0:
        return True
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    url = "https://raw.githubusercontent.com/sgossner/VSCO-2-CE/master/" + urllib.parse.quote(rel)
    tmp = dst + ".part"
    for attempt in range(4):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "daw-ai"})
            with urllib.request.urlopen(req, timeout=20) as r, open(tmp, "wb") as out:
                out.write(r.read())
            os.replace(tmp, dst)
            return True
        except Exception as e:
            if attempt == 3:
                print(f"WARN failed {rel}: {e}")
    return False

with concurrent.futures.ThreadPoolExecutor(max_workers=8) as ex:
    results = list(ex.map(download_one, needed))
print(f"VSCO-2-CE ready: {sum(results)}/{len(needed)} samples")
PY
convert_to_wav assets/sfz/vsco2

echo "=== 6. JJazzLab SoundFont (Jazz Tenor Sax, Brass Section, Latin Perc) ==="
if [ ! -s assets/soundfonts/JJazzLab-SoundFont.sf2 ]; then
  curl -C - -fL --retry 5 --retry-delay 2 --progress-bar -o assets/soundfonts/JJazzLab-SoundFont.sf2.part \
    "https://archive.org/download/jjazz-lab-sound-font/JJazzLab-SoundFont.sf2"
  mv assets/soundfonts/JJazzLab-SoundFont.sf2.part assets/soundfonts/JJazzLab-SoundFont.sf2
else
  echo "already present: assets/soundfonts/JJazzLab-SoundFont.sf2"
fi

echo "=== 7. Generate clean daw-core SFZ wrapper presets ==="
python3 - <<'PY'
import re

open("assets/sfz/meatbass/Caravan_Meatbass.sfz", "w").write("""<control>
set_cc107=0

<global>
ampeg_attack=0.002
ampeg_decay=0.0
ampeg_sustain=100
ampeg_release=0.22
volume=4

#include "Programs/pizz_basic_map.sfz"
""")

open("assets/sfz/weresax/Caravan_AltoSax.sfz", "w").write("""<global>
ampeg_attack=0.006
ampeg_decay=0.0
ampeg_sustain=100
ampeg_release=0.14
off_mode=fast
volume=4

#include "Programs/alto_map_piano_dynamic.sfz"
#include "Programs/alto_map_forte_dynamic.sfz"
""")

dyn_txt = open("assets/sfz/bear-sax/Programs/poly/dynfade_map.sfz").read()
out = [
    "<global>", "ampeg_attack=0.006", "ampeg_decay=0.0", "ampeg_sustain=100",
    "ampeg_release=0.15", "amp_veltrack=100", "volume=4", "",
]
cur_lo, cur_hi, cur_kc = 0, 127, 60
for raw_blk in dyn_txt.split("<region>")[1:]:
    m_s = re.search(r"sample=([^\s]+)", raw_blk)
    if not m_s:
        continue
    smp = m_s.group(1).replace("../", "")
    m_lo = re.search(r"lokey=(\d+)", raw_blk)
    m_hi = re.search(r"hikey=(\d+)", raw_blk)
    m_kc = re.search(r"pitch_keycenter=(\d+)", raw_blk)
    if m_lo: cur_lo = int(m_lo.group(1))
    if m_hi: cur_hi = int(m_hi.group(1))
    if m_kc: cur_kc = int(m_kc.group(1))
    lo = 28 if cur_lo == 36 else cur_lo
    hi = 72 if cur_hi == 68 else cur_hi
    lv, hv = (0, 68) if "_p.wav" in smp else (69, 127)
    out.extend([
        "<region>", f"sample={smp}", f"lokey={lo}", f"hikey={hi}",
        f"pitch_keycenter={cur_kc}", f"lovel={lv}", f"hivel={hv}", "",
    ])
open("assets/sfz/bear-sax/Caravan_BariSax.sfz", "w").write("\n".join(out))

tpt_txt = open("assets/sfz/vsco2/TrumpetSusVib.sfz").read()
tpt_txt = tpt_txt.replace("ampeg_release=0.5", "ampeg_release=0.20").replace("hikey=84", "hikey=92")
open("assets/sfz/vsco2/Caravan_Trumpet.sfz", "w").write(tpt_txt)

tbn_txt = open("assets/sfz/vsco2/TromboneSus.sfz").read()
tbn_txt = tbn_txt.replace("ampeg_release=0.6", "ampeg_release=0.22").replace("hikey=65", "hikey=78")
open("assets/sfz/vsco2/Caravan_Trombone.sfz", "w").write(tbn_txt)

def body_from_group(txt):
    return txt[txt.index("<group>"):]

stac_body = body_from_group(open("assets/sfz/vsco2/TromboneStac.sfz").read())
stac_body = re.sub(r"sample=([^\r\n]+)", r"sample=Brass/Tenor Trombone/stac/\1", stac_body)
stac_body = stac_body.replace("hikey=70", "hikey=74")

vib_body = body_from_group(open("assets/sfz/vsco2/TromboneVib.sfz").read())
vib_body = re.sub(r"sample=([^\r\n]+)", r"sample=Brass/Tenor Trombone/vib/\1", vib_body)
vib_body = vib_body.replace("lokey=41", "lokey=34").replace("hikey=67", "hikey=74")
vib_body = re.sub(r"volume=(-?\d+)", lambda m: f"volume={int(m.group(1)) + 2}", vib_body)

solo_sfz = "\n".join([
    "<control>",
    "set_cc74=100",
    "",
    "<global>",
    "group=1",
    "off_by=1",
    "off_mode=fast",
    "ampeg_attack=0.002",
    "ampeg_sustain=100",
    "",
    "<master>",
    "locc74=64",
    "hicc74=127",
    "ampeg_release=0.14",
    stac_body,
    "",
    "<master>",
    "locc74=0",
    "hicc74=63",
    "ampeg_attack=0.016",
    "ampeg_release=0.11",
    "offset=26460",
    vib_body,
])
open("assets/sfz/vsco2/Caravan_TromboneSolo.sfz", "w").write(solo_sfz)
PY

echo "Done fetching studio jazz assets:"
du -sh assets/sfz/* assets/soundfonts/* 2>/dev/null || true
