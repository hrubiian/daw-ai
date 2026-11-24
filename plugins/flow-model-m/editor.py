#!/usr/bin/env python3
"""Serve a Flow Music editor for one running daw-ai plugin slot."""

import argparse
import base64
import gzip
import hashlib
import io
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import struct
import subprocess
import time
import uuid


PLUGIN_ID = "local.daw-ai.flow.model-m-v4"
FLOW_ID_PREFIX = "local.daw-ai.fmx."
EDITOR_HASH = "7b2207829d1eca7e2b52bdaad248244296f60893af6119711297fb9fca46ac10"
MAX_SESSION_MESSAGE = 16 * 1024 * 1024
CURVE_POWERS = {"linear": 1, "squared": 2, "cubed": 3, "sqrt": 0.5, "cbrt": 1 / 3}
WINDOW_BROWSERS = (
    "google-chrome", "google-chrome-stable", "chromium", "chromium-browser",
    "firefox", "brave-browser", "brave", "vivaldi-stable", "vivaldi",
    "microsoft-edge", "microsoft-edge-stable", "opera",
)
DESKTOP_BROWSER_COMMANDS = {
    "com.google.Chrome": "google-chrome",
    "chromium_chromium": "chromium",
    "org.chromium.Chromium": "chromium",
    "firefox_firefox": "firefox",
    "org.mozilla.firefox": "firefox",
    "com.brave.Browser": "brave-browser",
}


class SessionError(Exception):
    """The live slot is gone or its settings could not be updated."""


class FlowEditor:
    def __init__(self, address, plugin, uid, flow_id):
        self.address = address
        self.plugin = plugin.resolve()
        self.uid = uid
        self.plugin_id = flow_id or PLUGIN_ID
        self.sandboxed = flow_id is not None
        if self.sandboxed:
            self.load_fmx()
        else:
            self.load_model_m()
        self.identity = None
        self.snapshot(self.call({"type": "inspect"}))

    def load_fmx(self):
        with self.plugin.open("rb") as source:
            raw = source.read(4 * 1024 * 1024 + 1)
        if len(raw) > 4 * 1024 * 1024 or self.plugin_id != FLOW_ID_PREFIX + hashlib.sha256(raw).hexdigest():
            raise SessionError("The FMX file has changed. Import it again from daw-ai.")
        bundle = json.loads(raw)
        self.name = bundle["name"]
        compressed = base64.b64decode(bundle["html"], validate=True)
        with gzip.GzipFile(fileobj=io.BytesIO(compressed)) as stream:
            document = stream.read(8 * 1024 * 1024 + 1)
        if len(document) > 8 * 1024 * 1024:
            raise SessionError("The FMX editor is too large.")
        self.html = document.decode()
        self.params = json.loads(bundle["metadata"])["params"]

    def load_model_m(self):
        self.name = "Model-M"
        document = self.plugin.with_suffix(".editor.html").read_bytes()
        if hashlib.sha256(document).hexdigest() != EDITOR_HASH:
            raise SessionError("The installed editor does not match the verified Model-M export.")
        self.html = document.decode().replace(
            'children: "CRT OSCILLOSCOPE"', 'children: "DAW OUTPUT SCOPE"'
        ).replace(
            "value.toFixed(displayPrecision(p.step))",
            'value.toFixed(p.unit === "ms" ? 1 : displayPrecision(p.step))',
        )
        path = self.plugin.with_suffix(".editor.json")
        self.params = json.loads(path.read_text())["params"]
        if len(self.params) != 23 or [p["id"] for p in self.params] != list(range(23)):
            raise SessionError("The Model-M parameter metadata is invalid.")

    def call(self, command, revision=None):
        request = {"version": 1, "operation_id": uuid.uuid4().hex,
                   "expected_revision": revision, "command": command}
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(2)
            client.connect(self.address)
            client.sendall(json.dumps(request).encode() + b"\n")
            with client.makefile("rb") as reply:
                return json.loads(reply.readline(MAX_SESSION_MESSAGE))

    def snapshot(self, reply):
        project = reply["project"]
        for track in project["tracks"]:
            for slot in [track["instrument"], *track.get("clap_fx", [])]:
                if slot.get("uid") != self.uid or slot.get("id") != self.plugin_id:
                    continue
                path = Path(reply["project_directory"]) / slot["path"]
                identity = (reply["project_directory"], project["project"]["name"], track["id"])
                if path.resolve() != self.plugin or self.identity not in (None, identity):
                    raise SessionError("The plugin changed. Reopen its editor from daw-ai.")
                self.identity = identity
                return track, slot
        raise SessionError("The plugin was removed. Reopen its editor from daw-ai.")

    def status(self, response):
        track, slot = self.snapshot(response)
        state = base64.b64decode(slot.get("state", ""), validate=True)
        count = len(self.params)
        version = b"FMX1" if self.sandboxed else b"MDM1"
        defaults = (struct.unpack(f"<{count}f", state[4:])
                    if len(state) == 4 + count * 4 and state[:4] == version
                    else [p["default"] for p in self.params])
        overrides = slot.get("params", {})
        values = [normalize(p, overrides.get(str(p["id"]), default))
                  for p, default in zip(self.params, defaults)]
        if self.sandboxed:
            return {"values": values}
        return {"values": values, "scope": response.get("master_scope"),
                "track": track["name"], "playing": response.get("playing", False)}

    def change(self, changes):
        if not isinstance(changes, dict) or not 1 <= len(changes) <= len(self.params):
            raise ValueError("Expected one or more parameter changes.")
        edits = []
        for key, normalized in changes.items():
            index = int(key)
            if str(index) != key or not 0 <= index < len(self.params):
                raise ValueError("Unknown instrument parameter.")
            if type(normalized) not in (int, float) or not 0 <= normalized <= 1:
                raise ValueError("Parameter value must be between 0 and 1.")
            edits.append({"type": "clap_param", "uid": self.uid, "param_id": index,
                          "value": denormalize(self.params[index], normalized)})
        before = self.call({"type": "inspect"})
        _, slot = self.snapshot(before)
        overrides = slot.get("params", {})
        edits = [e for e in edits if overrides.get(str(e["param_id"])) != e["value"]]
        if not edits:
            return self.status(before)
        response = self.call({"type": "batch", "label": self.name + " editor", "edits": edits}, before["revision"])
        if response.get("error"):
            raise SessionError(response["error"]["message"])
        return self.status(response)


def normalize(p, value):
    if not math.isfinite(value):
        value = p["default"]
    value = min(p["max"], max(p["min"], value))
    if p["min"] == p["max"]:
        return 0
    curve = p.get("curve", "linear")
    if curve == "exponential" and p["min"] > 0:
        return math.log(value / p["min"]) / math.log(p["max"] / p["min"])
    linear = (value - p["min"]) / (p["max"] - p["min"])
    return linear ** (1 / CURVE_POWERS.get(curve, 1))


def denormalize(p, normalized):
    curve = p.get("curve", "linear")
    if curve == "exponential" and p["min"] > 0:
        value = p["min"] * (p["max"] / p["min"]) ** normalized
    else:
        value = p["min"] + normalized ** CURVE_POWERS.get(curve, 1) * (p["max"] - p["min"])
    return math.floor(value + 0.5) if p.get("stepped") else value


def open_editor_window(url):
    default = ""
    try:
        result = subprocess.run(
            ["xdg-settings", "get", "default-web-browser"],
            capture_output=True, text=True, check=True, timeout=2,
        )
        desktop = result.stdout.strip().removesuffix(".desktop")
        default = DESKTOP_BROWSER_COMMANDS.get(desktop, desktop)
    except (OSError, subprocess.SubprocessError):
        pass

    candidates = dict.fromkeys((default, *WINDOW_BROWSERS)) if default in WINDOW_BROWSERS else WINDOW_BROWSERS
    for candidate in candidates:
        if executable := shutil.which(candidate):
            try:
                return subprocess.Popen(
                    [executable, "--new-window", url],
                    stdout=subprocess.DEVNULL, start_new_session=True,
                )
            except OSError:
                continue
    raise SessionError("Could not start a supported browser for the editor window.")


def serve(editor, javascript, open_browser, parent):
    token = "/" + secrets.token_urlsafe(32) + "/"
    script = "<script>" + javascript + "</script>"
    common_csp = (
        "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; "
        "img-src data: blob:; font-src data:; base-uri 'none'; form-action 'none'; "
    )
    if editor.sandboxed:
        # Both the iframe attribute and the response policy isolate the export
        # from the session API, including if someone opens the panel directly.
        document = (
            '<!doctype html><html><head><meta charset="UTF-8"><title>Flow Music — daw-ai</title>'
            '<style>html,body,iframe{width:100%;height:100%;border:0;margin:0;background:#151210}'
            'iframe{display:block}#status{position:fixed;bottom:1em;right:1em;padding:.5em;'
            'background:#5c320b;color:white;font:13px system-ui}</style>' + script
            + '</head><body><iframe title="Plugin editor" sandbox="allow-scripts" '
            'referrerpolicy="no-referrer" src="panel"></iframe><div id="status" role="status" '
            'hidden></div></body></html>'
        ).encode()
        head = re.search(r"<head(?:\s[^>]*)?>", editor.html, re.IGNORECASE)
        insert = head.end() if head else 0
        panel = (editor.html[:insert] + script + editor.html[insert:]).encode()
    else:
        header = (
            "<title>Model-M — daw-ai</title><style>"
            "body{margin:0;background:#151210}#root{display:grid;place-items:center;min-height:100vh}"
            "#root>:first-child{zoom:var(--model-m-zoom,1)}</style>" + script
        )
        document = editor.html.replace("<head>", "<head>" + header, 1).encode()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def send_body(self, status, body, mime="application/json", panel_response=False):
            self.send_response(status)
            self.send_header("Content-Type", mime)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("X-Content-Type-Options", "nosniff")
            policy = ("sandbox allow-scripts; frame-ancestors 'self'" if panel_response
                      else "connect-src 'self'; frame-src 'self'; frame-ancestors 'none'")
            self.send_header("Content-Security-Policy", common_csp + policy)
            try:
                self.end_headers()
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def check_path(self):
            if self.headers.get("Host") != authority or not self.path.startswith(token):
                self.send_error(404)
                return False
            return True

        def do_GET(self):
            if not self.check_path():
                return
            server.last_seen = time.monotonic()
            if self.path == token:
                self.send_body(200, document, "text/html; charset=utf-8")
            elif self.path == token + "panel" and editor.sandboxed:
                self.send_body(200, panel, "text/html; charset=utf-8", panel_response=True)
            elif self.path == token + "state":
                command = "inspect" if editor.sandboxed else "inspect_scope"
                self.request_json(lambda: editor.status(editor.call({"type": command})))
            else:
                self.send_error(404)

        def do_POST(self):
            if not self.check_path():
                return
            if self.path != token + "state" or self.headers.get("Origin") != origin:
                self.send_error(403)
                return
            length = self.headers.get("Content-Length", "")
            if not length.isdecimal() or not 0 < int(length) <= 16384:
                self.send_error(400)
                return
            if self.headers.get_content_type() != "application/json":
                self.send_error(415)
                return
            server.last_seen = time.monotonic()
            self.connection.settimeout(2)
            self.request_json(lambda: editor.change(json.loads(self.rfile.read(int(length)))))

        def request_json(self, operation):
            try:
                result, code = operation(), 200
            except SessionError as error:
                result, code = {"error": str(error)}, 409
            except OSError:
                result, code = {"error": "daw-ai session disconnected. Reopen the editor from daw-ai."}, 503
            except (ValueError, KeyError):
                result, code = {"error": "Could not read the instrument editor settings."}, 400
            self.send_body(code, json.dumps(result, allow_nan=False).encode())

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        authority = f"127.0.0.1:{server.server_port}"
        origin = f"http://{authority}"
        url = origin + token
        server.timeout = 1
        server.last_seen = time.monotonic()
        browser = open_editor_window(url) if open_browser else None
        if browser is None:
            print(url, flush=True)
        while time.monotonic() - server.last_seen < 90:
            if parent and os.getppid() != parent:
                break
            server.handle_request()
            if browser is not None and browser.poll() is not None:
                browser = None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", required=True)
    parser.add_argument("--plugin", type=Path, required=True)
    parser.add_argument("--slot", required=True)
    parser.add_argument("--flow-id")
    parser.add_argument("--open", action="store_true")
    parser.add_argument("--parent", type=int)
    parser.add_argument("--javascript")
    args = parser.parse_args()
    try:
        editor = FlowEditor(args.session, args.plugin, args.slot, args.flow_id)
        javascript = args.javascript or Path(__file__).with_suffix(".js").read_text()
        serve(editor, javascript, args.open, args.parent)
    except (OSError, ValueError, SessionError, KeyError) as error:
        parser.exit(1, f"Plugin editor: {error}\n")


if __name__ == "__main__":
    main()
