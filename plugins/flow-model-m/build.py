#!/usr/bin/env python3
"""Build a local CLAP for the Model-M v4 Flow Music export."""

import argparse
import base64
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

MODEL_M_DSP = "8c35088f7b5001e683e387f0713a2f66d1c4ec8742cd8a996c1db0f7a669083e"
MODEL_M_EDITOR = "7b2207829d1eca7e2b52bdaad248244296f60893af6119711297fb9fca46ac10"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fmx", type=Path)
    parser.add_argument("output", type=Path, help="destination .clap file")
    args = parser.parse_args()
    if not args.output.name.endswith(".clap"):
        parser.error("output must end in .clap")
    with args.fmx.open("rb") as source:
        raw = source.read(1_000_001)
    if len(raw) > 1_000_000:
        parser.error("FMX export too large")
    payload = json.loads(raw)
    if payload.get("type") != "vst_instrument":
        parser.error("expected an instrument export")
    compressed = base64.b64decode(payload["wasm"], validate=True)
    with gzip.GzipFile(fileobj=io.BytesIO(compressed)) as stream:
        dsp = stream.read(32_769)
    if hashlib.sha256(dsp).hexdigest() != MODEL_M_DSP:
        parser.error("the DSP differs from the Model-M v4 build with a verified memory layout")
    compressed_editor = base64.b64decode(payload["html"], validate=True)
    with gzip.GzipFile(fileobj=io.BytesIO(compressed_editor)) as stream:
        editor = stream.read(2_100_001)
    if hashlib.sha256(editor).hexdigest() != MODEL_M_EDITOR:
        parser.error("the editor differs from the verified Model-M v4 export")

    crate = Path(__file__).resolve().parent
    target = crate.parents[1] / "target" / "flow-model-m"
    target.mkdir(parents=True, exist_ok=True)
    assets = target / "payload"
    assets.mkdir(exist_ok=True)
    (assets / "model-m.wasm").write_bytes(dsp)
    (assets / "model-m.json").write_text(payload["metadata"], encoding="utf-8")
    env = os.environ | {"MODEL_M_PAYLOAD": str(assets), "CARGO_TARGET_DIR": str(target)}
    subprocess.run(["cargo", "build", "--release", "--locked"], cwd=crate, env=env, check=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.with_suffix(".editor.html").write_bytes(editor)
    args.output.with_suffix(".editor.json").write_text(payload["metadata"], encoding="utf-8")
    with tempfile.NamedTemporaryFile(dir=args.output.parent, delete=False) as temporary:
        staging = Path(temporary.name)
    try:
        shutil.copyfile(target / "release" / "libdaw_flow_model_m.so", staging)
        staging.chmod(0o755)
        staging.replace(args.output)
    finally:
        staging.unlink(missing_ok=True)
    print(args.output.resolve())


if __name__ == "__main__":
    main()
