from __future__ import annotations

import json
from dataclasses import asdict
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional

from .models import ContextEntry, ProjectContext, ProjectSnapshot, SessionContext, utcnow_iso


class ContextStore:
    def __init__(self, root: Path):
        self.root = root
        self.projects_dir = self.root / "projects"
        self.projects_dir.mkdir(parents=True, exist_ok=True)

    def _project_dir(self, project_id: str) -> Path:
        return self.projects_dir / project_id

    def _project_file(self, project_id: str) -> Path:
        return self._project_dir(project_id) / "project.json"

    def _snapshot_file(self, project_id: str) -> Path:
        return self._project_dir(project_id) / "snapshot.json"

    def _events_file(self, project_id: str) -> Path:
        return self._project_dir(project_id) / "events.jsonl"

    def _session_file(self, project_id: str) -> Path:
        return self._project_dir(project_id) / "sessions.json"

    def ensure_project(self, context: ProjectContext) -> None:
        directory = self._project_dir(context.project_id)
        directory.mkdir(parents=True, exist_ok=True)
        context.updated_at = utcnow_iso()
        self._project_file(context.project_id).write_text(
            json.dumps(asdict(context), indent=2),
            encoding="utf-8",
        )

        if not self._snapshot_file(context.project_id).exists():
            snapshot = ProjectSnapshot(
                project_id=context.project_id,
                summary=f"Project {context.name} initialized for: {context.goal}",
                status=context.status,
            )
            self.write_snapshot(snapshot)

        if not self._session_file(context.project_id).exists():
            self._session_file(context.project_id).write_text("{}", encoding="utf-8")

    def get_project(self, project_id: str) -> Optional[ProjectContext]:
        path = self._project_file(project_id)
        if not path.exists():
            return None
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["updated_at"] = payload.get("updated_at", utcnow_iso())
        return ProjectContext(**payload)

    def list_projects(self) -> List[str]:
        if not self.projects_dir.exists():
            return []
        return [item.name for item in self.projects_dir.iterdir() if item.is_dir()]

    def append_entry(self, entry: ContextEntry) -> None:
        directory = self._project_dir(entry.project_id)
        directory.mkdir(parents=True, exist_ok=True)
        entry.ts = utcnow_iso()
        with self._events_file(entry.project_id).open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(entry.to_dict(), ensure_ascii=False))
            handle.write("\n")

    def get_events(self, project_id: str, since_ts: Optional[str] = None, limit: int = 100) -> List[ContextEntry]:
        path = self._events_file(project_id)
        if not path.exists():
            return []

        lines = path.read_text(encoding="utf-8").splitlines()
        parsed = [ContextEntry.parse(json.loads(line)) for line in lines if line.strip()]
        if since_ts:
            parsed = [event for event in parsed if event.ts > since_ts]
        return parsed[-limit:]

    def write_snapshot(self, snapshot: ProjectSnapshot) -> None:
        snapshot.updated_at = utcnow_iso()
        self._snapshot_file(snapshot.project_id).write_text(
            json.dumps(snapshot.to_dict(), indent=2),
            encoding="utf-8",
        )

    def get_snapshot(self, project_id: str) -> Optional[ProjectSnapshot]:
        path = self._snapshot_file(project_id)
        if not path.exists():
            return None
        payload = json.loads(path.read_text(encoding="utf-8"))
        return ProjectSnapshot.parse(payload)

    def upsert_session(self, session: SessionContext) -> None:
        directory = self._project_dir(session.project_id)
        directory.mkdir(parents=True, exist_ok=True)
        session.updated_at = utcnow_iso()
        sessions = self._load_sessions(session.project_id)
        sessions[session.session_name] = session.__dict__
        self._session_file(session.project_id).write_text(
            json.dumps(sessions, indent=2, sort_keys=True),
            encoding="utf-8",
        )

    def get_session(self, project_id: str, session_name: str) -> Optional[SessionContext]:
        sessions = self._load_sessions(project_id)
        payload = sessions.get(session_name)
        if not payload:
            return None
        return SessionContext(**payload)

    def list_sessions(self, project_id: str) -> List[SessionContext]:
        sessions = self._load_sessions(project_id)
        return [SessionContext(**payload) for payload in sessions.values()]

    def _load_sessions(self, project_id: str) -> Dict[str, Dict[str, Any]]:
        path = self._session_file(project_id)
        if not path.exists():
            return {}
        payload = json.loads(path.read_text(encoding="utf-8"))
        return payload if isinstance(payload, dict) else {}

