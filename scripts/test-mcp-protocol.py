#!/usr/bin/env python3
"""MCP wire regression; run after cargo build -p daw-mcp. No audio device needed."""
from contextlib import contextmanager
import base64
import io
import json
import os
from pathlib import Path
import select
import struct
from socketserver import StreamRequestHandler, UnixStreamServer
import subprocess
import tempfile
from threading import Thread
import wave

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/debug/daw-mcp"
DEMO = ROOT / "examples/quickstart.daw.json"


@contextmanager
def mcp(arg, env=None):
    process = subprocess.Popen([str(BINARY), str(arg)], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, text=True, env=env)
    def send(message):
        process.stdin.write(json.dumps(message) + "\n")
        process.stdin.flush()
    def rpc(identifier, method, params):
        send(dict(jsonrpc="2.0", id=identifier, method=method, params=params))
        # Bound a broken server response without requiring third-party packages.
        assert select.select([process.stdout], [], [], 10)[0], "MCP response timed out"
        response = json.loads(process.stdout.readline())
        assert response.get("id") == identifier, response
        return response
    try:
        response = rpc(1, "initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="regression", version="1")))
        assert "result" in response, response
        send(dict(jsonrpc="2.0", method="notifications/initialized"))
        yield rpc
    finally:
        process.stdin.close()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


with tempfile.TemporaryDirectory(prefix="daw-mcp-protocol-") as directory:
    with mcp(Path(directory) / "song.daw.json") as rpc:
        response = rpc(2, "tools/list", {})
        names = {tool["name"] for tool in response["result"]["tools"]}
        assert {"session_state", "session_command", "start_listen", "cancel_job", "job_status", "rate_render", "compare_renders", "render_history", "set_sampler_variants", "apply_spatial_preset", "rescan_clap_plugins", "get_clap_params", "audition_excerpt", "prepare_audition", "record_audition_choice", "composition_guide", "analyze_melody", "set_volume_automation", "set_pattern_cc", "set_clip_gain"} <= names, names
        response = rpc(3, "tools/call", dict(name="project_summary", arguments={}))
        assert not response["result"].get("isError"), response
        response = rpc(4, "tools/call", dict(name="session_state", arguments={}))
        assert response["result"].get("isError"), response
        response = rpc(5, "tools/call", dict(name="render_history", arguments={}))
        assert not response["result"].get("isError"), response
        history = json.loads(response["result"]["content"][0]["text"])
        assert history == {"renders": [], "errors": [], "next_offset": None}, history
        response = rpc(6, "tools/call", dict(name="render_history", arguments={"limit": 101}))
        assert response["result"].get("isError"), response
        response = rpc(7, "tools/call", dict(name="job_status", arguments={"id": "../escape"}))
        assert response["result"].get("isError"), response
        response = rpc(8, "tools/call", dict(name="apply_spatial_preset", arguments={"preset": "tape_echo"}))
        assert not response["result"].get("isError"), response
        response = rpc(9, "tools/call", dict(name="get_project", arguments={}))
        project = json.loads(response["result"]["content"][0]["text"])
        assert project["returns"]["delay"]["sync"] == "eighth_dotted", project
        response = rpc(10, "tools/call", dict(name="set_returns", arguments={"params": {"delay": {"modulation_ms": 21}}}))
        assert response["result"].get("isError"), response
        response = rpc(11, "tools/call", dict(name="set_project", arguments={"json": DEMO.read_text()}))
        assert not response["result"].get("isError"), response
        comparison_args = {
            "reference": {"track_id": "drums", "pitch": 36},
            "target": {"track_id": "bass"},
            "start_bar": 0,
            "end_bar": 2,
        }
        response = rpc(12, "tools/call", dict(name="compare_groove", arguments=comparison_args))
        report = json.loads(response["result"]["content"][0]["text"])
        assert report["total"]["exact"] == 4 and report["total"]["target_onsets"] == 6, report
        assert report["bars"][0]["offsets"][2]["next_delta_ms"] < 0, report
        response = rpc(13, "tools/call", dict(name="analyze_melody", arguments={"track_id": "bass"}))
        assert not response["result"].get("isError"), response
        assert "melody (bass" in response["result"]["content"][0]["text"], response
        print("MCP initialization, history, error handling, groove comparison and melody analysis: passed")


with tempfile.TemporaryDirectory(prefix="daw-mcp-track-check-") as directory:
    song = Path(directory) / "tracks.daw.json"
    project = json.loads(DEMO.read_text())
    project["project"]["bpm"] = 120
    project["patterns"] = []
    project["arrangement"] = []
    project["tracks"] = [
        dict(id=track_id, name=track_id, type="audio", muted=muted, volume=1, pan=0,
             patterns=[], instrument=dict(type="audio_file", path=audio))
        for track_id, audio, muted in [
            ("stereo", "transition.wav", False),
            ("missing", "missing.wav", False),
            ("muted", "transition.wav", True),
        ]
    ]
    song.write_text(json.dumps(project))
    with wave.open(str(song.with_name("transition.wav")), "wb") as source:
        source.setparams((2, 2, 8000, 0, "NONE", "not compressed"))
        source.writeframes(struct.pack("<hh", 8000, -8000) * 16000)
        source.writeframes(struct.pack("<hh", 2000, -2000) * 16000)
    before = song.read_bytes()
    with mcp(song) as rpc:
        result = rpc(2, "tools/call", dict(name="check_tracks", arguments={"end_bar": 2}))["result"]
        assert not result.get("isError"), result
        review = json.loads(result["content"][0]["text"])
        stereo, missing, muted = review["tracks"]
        assert [t["status"] for t in (stereo, missing, muted)] == ["audible", "render_error", "muted"], review
        bars = stereo["measurements"]["bars"]
        assert [bar["bar"] for bar in bars] == [0, 1], bars
        assert all(bar["has_clip"] and not bar["quiet"] for bar in bars), bars
        assert bars[1]["change_from_previous_db"] < -10, bars
        assert stereo["measurements"]["stereo_correlation"] < -.99, stereo

        args = dict(track_id="muted", start_bar=1, duration_seconds=.25)
        result = rpc(3, "tools/call", dict(name="listen_track", arguments=args))["result"]
        assert not result.get("isError"), result
        text, audio = result["content"]
        audition = json.loads(text["text"])
        assert audition["project_hash"] == review["project_hash"], audition
        track = audition["tracks"][0]
        assert track["saved_muted"] and track["status"] == "audible", track
        assert track["measurements"]["bars"][0]["bar"] == 1, track
        assert audio["type"] == "audio" and audio["mimeType"] == "audio/wav", audio.keys()
        with wave.open(io.BytesIO(base64.b64decode(audio["data"])), "rb") as wav:
            assert (wav.getnchannels(), wav.getsampwidth(), wav.getnframes()) == (2, 2, 11025)
            left, right = struct.unpack("<hh", wav.readframes(1))
            assert left > 1000 and right < -1000, (left, right)
    assert song.read_bytes() == before
    assert not song.with_name("renders").exists()
    print("MCP track checks, render isolation, stereo excerpt and saved mute state: passed")


class SessionFixture(StreamRequestHandler):
    def handle(self):
        line = self.rfile.readline()
        if not line:
            return  # Socket discovery probe.
        request = json.loads(line)
        state = self.server.state
        command = request["command"]
        if command["type"] == "batch":
            assert request["expected_revision"] == state["revision"]
            state["project"] = command["edits"][0]["project"]
            state["revision"] += 1
            state["applied_revision"] = state["revision"]
        else:
            assert command["type"] == "inspect"
        self.wfile.write((json.dumps(dict(state, operation_id=request["operation_id"])) + "\n").encode())


@contextmanager
def live_session(sockets, name):
    project = json.loads(DEMO.read_text())
    project["project"]["name"] = name
    with UnixStreamServer(str(sockets / f"{name}.sock"), SessionFixture) as server:
        server.state = dict(version=1, project=project, revision=0, project_hash="test",
                            recent_edit="opened", project_directory=str(sockets.parent),
                            engine_status="applied", applied_revision=0, playback_tick=0,
                            playing=False, error=None)
        worker = Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
        worker.start()
        try:
            yield server.state
        finally:
            server.shutdown()
            worker.join()
    # Leave the closed socket in place to exercise stale-file discovery.


with tempfile.TemporaryDirectory(prefix="daw-mcp-live-") as directory:
    sockets = Path(directory) / f"daw-ai-{os.getuid()}"
    sockets.mkdir(mode=0o700)
    with mcp("--live", env=dict(os.environ, XDG_RUNTIME_DIR=directory)) as rpc:
        def call(identifier, name, **arguments):
            return rpc(identifier, "tools/call", dict(name=name, arguments=arguments))["result"]

        missing = call(2, "session_state")
        assert missing["isError"] and "No live DAW session" in missing["content"][0]["text"], missing
        with live_session(sockets, "First") as first:
            state = json.loads(call(3, "session_state")["content"][0]["text"])
            assert state["project"]["project"]["name"] == "First", state
            assert not call(4, "set_tempo", bpm=112).get("isError")
            first["project"]["project"]["name"] = "Opened in the same window"
            state = json.loads(call(5, "session_state")["content"][0]["text"])
            assert state["project"]["project"]["name"] == "Opened in the same window", state

        with live_session(sockets, "Second"):
            edit = call(6, "set_tempo", bpm=99)
            assert edit["isError"] and "session changed" in edit["content"][0]["text"], edit
            state = json.loads(call(7, "session_state")["content"][0]["text"])
            assert state["project"]["project"]["name"] == "Second" and state["revision"] == 0, state
            assert not call(8, "set_tempo", bpm=106).get("isError")
            with live_session(sockets, "Third"):
                ambiguous = call(9, "session_state")
                assert ambiguous["isError"] and "Multiple live DAW sessions" in ambiguous["content"][0]["text"], ambiguous
            state = json.loads(call(10, "session_state")["content"][0]["text"])
            assert state["project"]["project"]["bpm"] == 106, state
        print("MCP automatic live connection, restart, and multiple-session handling: passed")
