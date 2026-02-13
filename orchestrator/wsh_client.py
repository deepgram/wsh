from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Dict, List, Optional
from urllib import request, parse, error


class WshClientError(RuntimeError):
    pass


@dataclass
class WshSession:
    name: str


class WshApiClient:
    def __init__(self, base_url: str, token: Optional[str] = None):
        self.base_url = base_url.rstrip("/")
        self.token = token

    def _request(
        self,
        method: str,
        path: str,
        body: Optional[bytes] = None,
        query: Optional[Dict[str, Any]] = None,
        content_type: str = "application/json",
    ) -> bytes:
        url = f"{self.base_url}{path}"
        if query:
            url += "?" + parse.urlencode(query)
        req = request.Request(url, data=body, method=method)
        if content_type:
            req.add_header("Content-Type", content_type)
        if self.token:
            req.add_header("Authorization", f"Bearer {self.token}")
        try:
            with request.urlopen(req) as resp:
                return resp.read()
        except error.HTTPError as exc:
            detail = exc.read().decode(errors="replace")
            raise WshClientError(f"{method} {url} failed with {exc.code}: {detail}")

    @staticmethod
    def _json_load(payload: bytes) -> Any:
        if not payload:
            return None
        return json.loads(payload.decode("utf-8"))

    def list_sessions(self) -> List[Dict[str, Any]]:
        payload = self._request("GET", "/sessions")
        return self._json_load(payload)

    def create_session(self, name: str | None = None) -> Dict[str, Any]:
        body = {} if name is None else {"name": name}
        if name:
            body["name"] = name
        return self._json_load(self._request("POST", "/sessions", body=json.dumps(body).encode("utf-8")))

    def get_screen(self, session: str, fmt: str = "styled") -> Dict[str, Any]:
        payload = self._request("GET", f"/sessions/{parse.quote(session, safe='')}/screen", query={"format": fmt})
        return self._json_load(payload)

    def get_scrollback(self, session: str, fmt: str = "styled", offset: int = 0, limit: int = 100) -> Dict[str, Any]:
        payload = self._request(
            "GET",
            f"/sessions/{parse.quote(session, safe='')}/scrollback",
            query={"format": fmt, "offset": offset, "limit": limit},
        )
        return self._json_load(payload)

    def send_input(self, session: str, data: str) -> None:
        self._request(
            "POST",
            f"/sessions/{parse.quote(session, safe='')}/input",
            body=data.encode("utf-8"),
            content_type="text/plain",
        )

    def quiesce(
        self,
        session: str,
        timeout_ms: int = 500,
        max_wait_ms: int = 10_000,
        fresh: bool = False,
    ) -> Dict[str, Any]:
        payload = self._request(
            "GET",
            f"/sessions/{parse.quote(session, safe='')}/quiesce",
            query={"timeout_ms": timeout_ms, "max_wait_ms": max_wait_ms, "fresh": str(fresh).lower()},
        )
        return self._json_load(payload)

    def delete_session(self, session: str) -> bool:
        payload = self._request("DELETE", f"/sessions/{parse.quote(session, safe='')}")
        response = self._json_load(payload)
        if isinstance(response, dict):
            return bool(response.get("ok", True))
        return True
