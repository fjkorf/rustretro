"""Shared HTTP client for RustRetro's streamable-HTTP MCP server.

Before this module existed, the same ~30 lines (initialize handshake,
`mcp-session-id` header tracking, `tools/call`, `resources/read`, parsing the
SSE `data:` line out of the response body) had been reimplemented from
scratch at least five times: `shadow/play.py`'s `McpClient`,
`scripts/re/hold_fight.py`'s free functions, `scripts/re/execmap.py`'s `Mcp`,
`scripts/re/asura_assets.py`'s `Mcp`, and assorted one-off throwaways. This is
the one implementation; everything else should import it.

The handshake is automatic: the first `call()`/`read_resource()` triggers
`connect()` if it hasn't happened yet, so callers can't hit the "forgot to
initialize -> 422" mistake that bit this codebase twice.
"""

from __future__ import annotations

import base64
import json
import urllib.request

__all__ = ["McpClient", "McpError"]


class McpError(RuntimeError):
    """A tool call failed, or the server returned no usable result."""


class McpClient:
    def __init__(self, url: str, *, timeout: float = 15.0,
                 client_name: str = "shadow-mcp-client"):
        self.url = url
        self.timeout = timeout
        self.client_name = client_name
        self.sid: str | None = None
        self._req_id = 0
        self._connected = False

    # ── transport ────────────────────────────────────────────────────────
    def _post(self, payload: dict) -> dict | None:
        req = urllib.request.Request(
            self.url, data=json.dumps(payload).encode(), method="POST"
        )
        req.add_header("Content-Type", "application/json")
        req.add_header("Accept", "application/json, text/event-stream")
        if self.sid:
            req.add_header("mcp-session-id", self.sid)
        with urllib.request.urlopen(req, timeout=self.timeout) as resp:
            self.sid = resp.headers.get("mcp-session-id", self.sid)
            body = resp.read().decode()
        for line in body.splitlines():
            if line.startswith("data:") and line[5:].strip():
                return json.loads(line[5:].strip())
        return None

    def connect(self, url: str | None = None) -> "McpClient":
        """Run the initialize handshake. Optional; `call()`/`read_resource()`
        do this automatically on first use if you don't."""
        if url is not None:
            self.url = url
        self._post({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05", "capabilities": {},
                "clientInfo": {"name": self.client_name, "version": "1"},
            },
        })
        self._post({"jsonrpc": "2.0", "method": "notifications/initialized"})
        self._connected = True
        return self

    def _ensure_connected(self) -> None:
        if not self._connected:
            self.connect()

    # ── MCP protocol ─────────────────────────────────────────────────────
    def call(self, tool: str, **args) -> dict:
        """Call an MCP tool and return its parsed JSON result payload."""
        self._ensure_connected()
        self._req_id += 1
        resp = self._post({
            "jsonrpc": "2.0", "id": self._req_id, "method": "tools/call",
            "params": {"name": tool, "arguments": args},
        })
        if resp is None or "result" not in resp:
            raise McpError(f"{tool}: no result in response ({resp!r})")
        result = resp["result"]
        if result.get("isError"):
            raise McpError(f"{tool}: {result}")
        return json.loads(result["content"][0]["text"])

    def read_resource(self, uri: str) -> bytes:
        """Return the raw (base64-decoded) blob of an MCP resource."""
        self._ensure_connected()
        resp = self._post({
            "jsonrpc": "2.0", "id": 1, "method": "resources/read",
            "params": {"uri": uri},
        })
        if resp is None or "result" not in resp:
            raise McpError(f"resources/read {uri}: no result in response ({resp!r})")
        blob = resp["result"]["contents"][0]["blob"]
        return base64.b64decode(blob)

    # ── convenience wrappers over common tools ──────────────────────────
    def read_memory(self, addr: int, length: int) -> bytes:
        r = self.call("read_memory", addr=addr, len=length)
        if "error" in r:
            raise McpError(f"read_memory(0x{addr:X}, {length}): {r['error']}")
        return bytes.fromhex(r["hex"].replace(" ", ""))

    def press(self, buttons: list[str], frames: int = 8, port: int = 0) -> dict:
        return self.call("press_buttons", buttons=buttons, frames=frames, port=port)

    def enable_writes(self) -> dict:
        return self.call("enable_writes")

    def write_memory(self, addr: int, length: int, value: int) -> dict:
        return self.call("write_memory", addr=addr, len=length, value=value)

    def load_state(self, spec: str) -> dict:
        """`spec` is a save-state slot number (as a string) or a file path."""
        try:
            slot = int(spec)
            return self.call("load_state", slot=slot)
        except ValueError:
            return self.call("load_state", path=spec)

    def pause(self) -> dict:
        return self.call("pause")

    def resume(self) -> dict:
        return self.call("resume")

    def screenshot(self, path: str) -> None:
        """Fetch `app://screen` (a PNG) and write it to `path`."""
        png = self.read_resource("app://screen")
        with open(path, "wb") as f:
            f.write(png)
