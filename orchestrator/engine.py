from __future__ import annotations

import json
import time
from dataclasses import asdict
from pathlib import Path
from typing import Iterable, List, Optional

from .config import OrchestratorConfig
from .models import ContextEntry, EventKind, ProjectContext, ProjectSnapshot, SessionContext
from .store import ContextStore
from .wsh_client import WshApiClient, WshClientError


class Orchestrator:
    def __init__(self, config: OrchestratorConfig):
        self.config = config
        self.client = WshApiClient(base_url=config.wsh_base_url, token=config.token)
        self.store = ContextStore(root=Path(config.state_dir))

    def ensure_project(self, project_id: str, name: str, goal: str, branch: Optional[str] = None) -> ProjectContext:
        project = ProjectContext(
            project_id=project_id,
            name=name,
            goal=goal,
            status="active",
            active_branch=branch,
        )
        self.store.ensure_project(project)
        self.store.append_entry(
            ContextEntry(
                project_id=project_id,
                session_name="orchestrator",
                actor="system",
                kind=EventKind.STATUS,
                text=f"Project '{project_id}' initialized/updated for goal: {goal}",
            )
        )
        return project

    def create_session(self, project_id: str, role: str, session_name: Optional[str] = None) -> str:
        if session_name is None:
            session_name = self._next_session_name(project_id, role)
        created = self.client.create_session(session_name)
        resolved_name = created.get("name", session_name)

        project = self.store.get_project(project_id)
        if project is None:
            raise RuntimeError(f"Project '{project_id}' not found. Create it first.")

        self.store.upsert_session(
            SessionContext(
                project_id=project_id,
                session_name=resolved_name,
                role=role,
                state="running",
                goal=project.goal,
                next_action="waiting",
            )
        )

        self.store.append_entry(
            ContextEntry(
                project_id=project_id,
                session_name=resolved_name,
                actor="orchestrator",
                kind=EventKind.HANDOFF,
                text=f"Session '{resolved_name}' created for role '{role}'.",
            )
        )
        return resolved_name

    def dispatch_command(
        self,
        project_id: str,
        session_name: str,
        command: str,
        heartbeat: bool = True,
    ) -> str:
        self._require_session(project_id, session_name)
        self.store.upsert_session(
            SessionContext(
                **asdict(self._require_session(project_id, session_name)),
                state="running",
                last_signal=f"executing: {command}",
                next_action="send_input",
            )
        )
        self.store.append_entry(
            ContextEntry(
                project_id=project_id,
                session_name=session_name,
                actor="orchestrator",
                kind=EventKind.STATUS,
                text=f"Dispatching command: {command}",
            )
        )

        self.client.send_input(session_name, command + "\n")
        quiesce = self.client.quiesce(session_name, timeout_ms=500, max_wait_ms=10000, fresh=True)

        screen = self.client.get_screen(session_name, fmt="plain")
        first_line = None
        if isinstance(screen, dict):
            lines = screen.get("lines")
            if isinstance(lines, list) and lines:
                first_line = lines[0]

        self.store.append_entry(
            ContextEntry(
                project_id=project_id,
                session_name=session_name,
                actor="orchestrator",
                kind=EventKind.NOTE,
                text=self._summarize_observation(command, quiesce, first_line),
                refs={"quiesce": quiesce, "screen": self._first_line_to_value(first_line)},
            )
        )
        self.store.upsert_session(
            SessionContext(
                **asdict(self._require_session(project_id, session_name)),
                state="idle",
                last_signal="completed_dispatch",
                next_action="awaiting_task",
            )
        )
        if heartbeat:
            snapshot = self._build_snapshot(project_id)
            self.store.write_snapshot(snapshot)
        return json.dumps(quiesce, ensure_ascii=False)

    def run_task(
        self,
        project_id: str,
        role: str,
        commands: List[str],
        session_name: Optional[str] = None,
        heartbeat_interval: Optional[float] = None,
    ) -> List[str]:
        heartbeat_interval = self.config.poll_interval_seconds if heartbeat_interval is None else heartbeat_interval
        worker_name = self.create_session(project_id, role=role, session_name=session_name)
        responses: List[str] = []
        for index, command in enumerate(commands):
            responses.append(self.dispatch_command(project_id, worker_name, command))
            if heartbeat_interval > 0:
                self.store.append_entry(
                    ContextEntry(
                        project_id=project_id,
                        session_name=worker_name,
                        actor="system",
                        kind=EventKind.STATUS,
                        text=f"Heartbeat after command #{index + 1}: session={worker_name}, role={role}",
                    )
                )
                self.store.write_snapshot(self._build_snapshot(project_id))
            time.sleep(max(0, heartbeat_interval))
        self.store.append_entry(
            ContextEntry(
                project_id=project_id,
                session_name=worker_name,
                actor="orchestrator",
                kind=EventKind.STATUS,
                text=f"Task complete for session '{worker_name}'.",
            )
        )
        return responses

    def project_report(self, project_id: str) -> dict:
        snapshot = self.store.get_snapshot(project_id)
        events = self.store.get_events(project_id, limit=20)
        sessions = self.store.list_sessions(project_id)
        return {
            "snapshot": snapshot.to_dict() if snapshot else None,
            "sessions": [asdict(session) for session in sessions],
            "recent_events": [event.to_dict() for event in events],
        }

    def pull_session(self, project_id: str, session_name: str) -> dict:
        self._require_session(project_id, session_name)
        screen = self.client.get_screen(session_name, "styled")
        scrollback = self.client.get_scrollback(session_name)
        return {
            "project_id": project_id,
            "session_name": session_name,
            "screen": screen,
            "scrollback": scrollback,
        }

    def list_wsh_sessions(self) -> List[dict]:
        sessions = self.client.list_sessions()
        if isinstance(sessions, list):
            return sessions
        return []

    def _build_snapshot(self, project_id: str) -> ProjectSnapshot:
        project = self.store.get_project(project_id)
        if not project:
            raise RuntimeError(f"Project '{project_id}' not found")

        events = self.store.get_events(project_id, limit=5)
        blockers = [entry.text for entry in events if entry.kind_value == EventKind.ERROR]
        highlights = [entry.text for entry in events if entry.human_attention_needed or entry.kind_value == EventKind.APPROVAL]
        next_steps = [entry.text for entry in events if entry.kind_value == EventKind.STATUS]

        return ProjectSnapshot(
            project_id=project_id,
            summary=f"{project.name} status: {project.status}",
            status=project.status,
            open_blockers=blockers[:5],
            next_steps=next_steps[:10],
            recent_highlights=highlights[:10],
        )

    def _require_session(self, project_id: str, session_name: str) -> SessionContext:
        session = self.store.get_session(project_id, session_name)
        if not session:
            raise RuntimeError(f"Session '{session_name}' not tracked under project '{project_id}'")
        return session

    def _next_session_name(self, project_id: str, role: str) -> str:
        sessions = self.store.list_sessions(project_id)
        count = 1
        existing = {session.session_name for session in sessions}
        while True:
            candidate = f"{project_id}-{role}-{count:03d}"
            if candidate not in existing:
                return candidate
            count += 1

    @staticmethod
    def _summarize_observation(command: str, quiesce: dict, first_line: object) -> str:
        first = ""
        if first_line is not None:
            first = str(first_line).replace("\n", " ")
            if len(first) > 120:
                first = first[:120] + "…"
        return f"Ran '{command}'. Session state: {quiesce}. First line now: {first}"

    @staticmethod
    def _first_line_to_value(first_line: object) -> dict:
        if isinstance(first_line, dict):
            return first_line
        return {"value": first_line}
