#!/usr/bin/env python3
"""MCP wire regression; run after cargo build -p daw-mcp. No audio device needed."""
import json
from pathlib import Path
import subprocess
import tempfile

with tempfile.TemporaryDirectory(prefix="daw-mcp-protocol-") as directory:
    binary = Path(__file__).resolve().parents[1] / "target/debug/daw-mcp"
    process = subprocess.Popen([str(binary), str(Path(directory) / "song.daw.json")],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    def send(message):
        process.stdin.write(json.dumps(message) + "\n")
        process.stdin.flush()
    def rpc(identifier, method, params):
        send(dict(jsonrpc="2.0", id=identifier, method=method, params=params))
        # Bound a broken server response without requiring third-party packages.
        import select
        assert select.select([process.stdout], [], [], 10)[0], "MCP response timed out"
        response = json.loads(process.stdout.readline())
        assert response.get("id") == identifier, response
        return response
    try:
        response = rpc(1, "initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="regression", version="1")))
        assert "result" in response, response
        send(dict(jsonrpc="2.0", method="notifications/initialized"))
        response = rpc(2, "tools/list", {})
        names = {tool["name"] for tool in response["result"]["tools"]}
        assert {"session_state", "session_command", "start_listen", "cancel_job", "job_status", "rate_render", "compare_renders", "render_history", "set_sampler_variants", "apply_spatial_preset", "rescan_clap_plugins", "get_clap_params"} <= names, names
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
        print("MCP initialization, tools, offline summary, history and error handling: passed")
    finally:
        process.stdin.close()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
