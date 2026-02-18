"""Asyncio HTTP + WebSocket server for the orchestrator queue.

Exposes the orchestrator's event queue via HTTP REST endpoints and
pushes real-time updates over WebSocket. Zero external dependencies.
"""
from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import struct
from pathlib import Path
from typing import List, Optional, Set

from .config import OrchestratorConfig
from .models import ContextEntry
from .store import ContextStore

# WebSocket constants
WS_GUID = "258EAFA5-E914-47DA-95CA-5AB9FBF10A37"
OP_TEXT = 0x1
OP_CLOSE = 0x8
OP_PING = 0x9
OP_PONG = 0xA


# --- WebSocket protocol helpers ---

def ws_accept_key(key: str) -> str:
    digest = hashlib.sha1((key + WS_GUID).encode()).digest()
    return base64.b64encode(digest).decode()


async def ws_read_frame(reader: asyncio.StreamReader) -> tuple[int, bytes]:
    header = await reader.readexactly(2)
    opcode = header[0] & 0xF
    masked = (header[1] >> 7) & 1
    length = header[1] & 0x7F

    if length == 126:
        data = await reader.readexactly(2)
        length = struct.unpack("!H", data)[0]
    elif length == 127:
        data = await reader.readexactly(8)
        length = struct.unpack("!Q", data)[0]

    mask = await reader.readexactly(4) if masked else None
    payload = await reader.readexactly(length)

    if mask:
        payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))

    return opcode, payload


async def ws_write_frame(writer: asyncio.StreamWriter, opcode: int, payload: bytes) -> None:
    frame = bytearray()
    frame.append(0x80 | opcode)

    length = len(payload)
    if length < 126:
        frame.append(length)
    elif length < 65536:
        frame.append(126)
        frame.extend(struct.pack("!H", length))
    else:
        frame.append(127)
        frame.extend(struct.pack("!Q", length))

    frame.extend(payload)
    writer.write(bytes(frame))
    await writer.drain()


class WebSocketConnection:
    """A single WebSocket client connection."""

    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
        self.reader = reader
        self.writer = writer
        self.closed = False

    async def send(self, message: str) -> None:
        if self.closed:
            return
        try:
            await ws_write_frame(self.writer, OP_TEXT, message.encode())
        except Exception:
            self.closed = True

    async def recv(self) -> Optional[str]:
        try:
            opcode, payload = await ws_read_frame(self.reader)
            if opcode == OP_CLOSE:
                self.closed = True
                return None
            if opcode == OP_PING:
                await ws_write_frame(self.writer, OP_PONG, payload)
                return await self.recv()
            if opcode == OP_TEXT:
                return payload.decode()
            return None
        except Exception:
            self.closed = True
            return None

    async def close(self) -> None:
        if not self.closed:
            self.closed = True
            try:
                await ws_write_frame(self.writer, OP_CLOSE, b"")
                self.writer.close()
            except Exception:
                pass


class QueueServer:
    """HTTP + WebSocket server for the orchestrator queue."""

    def __init__(self, store: ContextStore, host: str = "127.0.0.1", port: int = 9090):
        self.store = store
        self.host = host
        self.port = port
        self.ws_clients: Set[WebSocketConnection] = set()
        self._known_queue_ids: set[str] = set()

    async def start(self) -> None:
        server = await asyncio.start_server(self._handle_connection, self.host, self.port)
        print(f"Orchestrator queue server on http://{self.host}:{self.port}")
        print(f"  HTTP:      GET /queue, POST /queue/:id/resolve, GET /projects")
        print(f"  WebSocket: /ws")

        poll_task = asyncio.create_task(self._poll_queue())

        try:
            async with server:
                await server.serve_forever()
        finally:
            poll_task.cancel()

    async def _poll_queue(self) -> None:
        """Poll the store for queue changes and broadcast diffs."""
        while True:
            await asyncio.sleep(2)
            try:
                current = self.store.get_queue()
                current_ids = {e.id for e in current}

                for entry in current:
                    if entry.id not in self._known_queue_ids:
                        await self._broadcast(json.dumps({
                            "type": "queue_add",
                            "entry": entry.to_dict(),
                        }))

                for old_id in self._known_queue_ids - current_ids:
                    await self._broadcast(json.dumps({
                        "type": "queue_remove",
                        "id": old_id,
                    }))

                self._known_queue_ids = current_ids
            except Exception as e:
                print(f"Poll error: {e}")

    async def _broadcast(self, message: str) -> None:
        dead: set[WebSocketConnection] = set()
        for client in self.ws_clients:
            try:
                await client.send(message)
            except Exception:
                dead.add(client)
        self.ws_clients -= dead

    # --- Connection handling ---

    async def _handle_connection(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ) -> None:
        try:
            request_line, headers, body_start = await self._read_http_request(reader)
            if not request_line:
                writer.close()
                return

            parts = request_line.split(" ", 2)
            if len(parts) < 2:
                writer.close()
                return
            method, path = parts[0], parts[1]

            if headers.get("upgrade", "").lower() == "websocket":
                await self._handle_ws_upgrade(reader, writer, headers)
                return

            await self._handle_http(method, path, reader, writer, headers, body_start)
        except Exception as e:
            print(f"Connection error: {e}")
        finally:
            try:
                writer.close()
            except Exception:
                pass

    async def _read_http_request(
        self, reader: asyncio.StreamReader
    ) -> tuple[Optional[str], dict[str, str], bytes]:
        data = b""
        while b"\r\n\r\n" not in data:
            chunk = await reader.read(8192)
            if not chunk:
                return None, {}, b""
            data += chunk

        header_end = data.index(b"\r\n\r\n")
        header_part = data[:header_end].decode()
        body_start = data[header_end + 4:]

        lines = header_part.split("\r\n")
        request_line = lines[0]
        headers: dict[str, str] = {}
        for line in lines[1:]:
            if ": " in line:
                key, value = line.split(": ", 1)
                headers[key.lower()] = value

        return request_line, headers, body_start

    # --- HTTP routing ---

    async def _handle_http(
        self,
        method: str,
        path: str,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
        headers: dict[str, str],
        body_start: bytes,
    ) -> None:
        if method == "OPTIONS":
            await self._send_response(writer, 204, b"")
            return

        clean_path = path.split("?", 1)[0].rstrip("/")

        if clean_path == "/queue" and method == "GET":
            entries = self.store.get_queue()
            body = json.dumps([e.to_dict() for e in entries]).encode()
            await self._send_response(writer, 200, body)

        elif clean_path.startswith("/queue/") and method == "POST":
            entry_id = clean_path.split("/")[2]
            body_data = await self._read_body(reader, headers, body_start)
            try:
                payload = json.loads(body_data) if body_data else {}
            except json.JSONDecodeError:
                await self._send_response(writer, 400, b'{"error":"invalid json"}')
                return

            result = self._resolve_entry(entry_id, payload)
            if result:
                await self._send_response(writer, 200, json.dumps(result).encode())
            else:
                await self._send_response(writer, 404, b'{"error":"entry not found"}')

        elif clean_path == "/projects" and method == "GET":
            projects = []
            for pid in self.store.list_projects():
                project = self.store.get_project(pid)
                if project:
                    sessions = self.store.list_sessions(pid)
                    projects.append({
                        "project_id": pid,
                        "name": project.name,
                        "status": project.status,
                        "session_count": len(sessions),
                    })
            await self._send_response(writer, 200, json.dumps(projects).encode())

        elif (clean_path.startswith("/projects/")
              and clean_path.endswith("/sessions")
              and method == "GET"):
            project_id = clean_path.split("/")[2]
            sessions = self.store.list_sessions(project_id)
            body = json.dumps([s.__dict__ for s in sessions]).encode()
            await self._send_response(writer, 200, body)

        else:
            await self._send_response(writer, 404, b'{"error":"not found"}')

    async def _read_body(
        self,
        reader: asyncio.StreamReader,
        headers: dict[str, str],
        body_start: bytes,
    ) -> bytes:
        content_length = int(headers.get("content-length", "0"))
        if content_length == 0:
            return body_start
        body = body_start
        while len(body) < content_length:
            chunk = await reader.read(content_length - len(body))
            if not chunk:
                break
            body += chunk
        return body

    def _resolve_entry(self, entry_id: str, payload: dict) -> Optional[dict]:
        action = payload.get("action", "acknowledge")
        text = payload.get("text", "")

        for project_id in self.store.list_projects():
            for event in self.store.get_events(project_id, limit=1000):
                if event.id == entry_id:
                    self.store.resolve_entry(project_id, entry_id, action, text)
                    return {"resolved": True, "id": entry_id, "action": action}
        return None

    # --- HTTP response ---

    async def _send_response(
        self,
        writer: asyncio.StreamWriter,
        status: int,
        body: bytes,
        content_type: str = "application/json",
    ) -> None:
        status_text = {
            200: "OK", 204: "No Content", 400: "Bad Request", 404: "Not Found",
        }.get(status, "OK")
        lines = [
            f"HTTP/1.1 {status} {status_text}",
            f"Content-Type: {content_type}",
            f"Content-Length: {len(body)}",
            "Access-Control-Allow-Origin: *",
            "Access-Control-Allow-Methods: GET, POST, OPTIONS",
            "Access-Control-Allow-Headers: Content-Type",
            "Connection: close",
            "",
            "",
        ]
        writer.write("\r\n".join(lines).encode() + body)
        await writer.drain()

    # --- WebSocket upgrade ---

    async def _handle_ws_upgrade(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
        headers: dict[str, str],
    ) -> None:
        key = headers.get("sec-websocket-key", "")
        accept = ws_accept_key(key)

        response = (
            "HTTP/1.1 101 Switching Protocols\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Accept: {accept}\r\n"
            "Access-Control-Allow-Origin: *\r\n"
            "\r\n"
        )
        writer.write(response.encode())
        await writer.drain()

        conn = WebSocketConnection(reader, writer)
        self.ws_clients.add(conn)

        # Send current queue state on connect
        entries = self.store.get_queue()
        await conn.send(json.dumps({
            "type": "queue_snapshot",
            "entries": [e.to_dict() for e in entries],
        }))

        try:
            while not conn.closed:
                msg = await conn.recv()
                if msg is None:
                    break
        finally:
            self.ws_clients.discard(conn)
            await conn.close()


def run_server(config: OrchestratorConfig, host: str = "127.0.0.1", port: int = 9090) -> None:
    store = ContextStore(root=Path(config.state_dir))
    server = QueueServer(store, host=host, port=port)
    asyncio.run(server.start())
