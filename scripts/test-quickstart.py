#!/usr/bin/env python3
"""Exercise the documented demo from a directory without any local assets."""

from array import array
import math
from pathlib import Path
import shutil
import subprocess
import tempfile


repo = Path(__file__).resolve().parents[1]
binary = repo / "target/debug/daw-ai"
demo = Path("examples/quickstart.daw.json")

with tempfile.TemporaryDirectory(prefix="daw-quickstart-") as directory:
    root = Path(directory)
    (root / demo.parent).mkdir()
    shutil.copyfile(repo / demo, root / demo)
    shutil.copyfile(repo / "fetch-assets.sh", root / "fetch-assets.sh")
    subprocess.run(["bash", "fetch-assets.sh", "--quickstart"], cwd=root, check=True, timeout=30)
    subprocess.run([binary, "doctor", "--headless", demo], cwd=root, check=True, timeout=30)
    subprocess.run([binary, demo, "--export-wav", "first-light.wav"], cwd=root, check=True, timeout=60)

    raw = subprocess.check_output(
        ["ffmpeg", "-v", "error", "-i", "first-light.wav", "-f", "f32le", "-"],
        cwd=root,
        timeout=30,
    )
    samples = array("f")
    samples.frombytes(raw)
    assert all(map(math.isfinite, samples)), "demo contains non-finite samples"
    # The score lasts eight 4/4 bars at 104 BPM, followed by the effect tail.
    bar_samples = round(44_100 * 240 / 104) * 2
    assert len(samples) >= 8 * bar_samples, "demo truncated before bar eight"
    for index in range(8):
        bar = samples[index * bar_samples : (index + 1) * bar_samples]
        assert max(map(abs, bar)) > 0.01, f"demo is silent in bar {index + 1}"
    print("Quickstart assets, doctor, and eight audible bars passed")
