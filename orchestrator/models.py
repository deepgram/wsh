from __future__ import annotations

from dataclasses import dataclass, field, asdict
from datetime import datetime, timezone
from enum import Enum
from typing import Any, Dict, List, Optional
import json
import uuid


def utcnow_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


class EventKind(str, Enum):
    STATUS = "status"
    HANDOFF = "handoff"
    NOTE = "note"
    ERROR = "error"
    APPROVAL = "approval_needed"


@dataclass
class ProjectContext:
    project_id: str
    name: str
    goal: str
    status: str = "active"
    default_cwd: Optional[str] = None
    active_branch: Optional[str] = None
    owner: Optional[str] = None
    updated_at: str = field(default_factory=utcnow_iso)


@dataclass
class SessionContext:
    project_id: str
    session_name: str
    role: str
    agent_id: Optional[str] = None
    state: str = "idle"
    goal: Optional[str] = None
    next_action: Optional[str] = None
    last_signal: Optional[str] = None
    updated_at: str = field(default_factory=utcnow_iso)


@dataclass
class ContextEntry:
    project_id: str
    session_name: str
    actor: str
    kind: EventKind | str
    text: str
    ts: str = field(default_factory=utcnow_iso)
    refs: Dict[str, Any] = field(default_factory=dict)
    human_attention_needed: bool = False
    id: Optional[str] = None

    def __post_init__(self) -> None:
        if isinstance(self.kind, str):
            self.kind = EventKind(self.kind) if self.kind in set(k.value for k in EventKind) else self.kind
        if self.id is None:
            self.id = str(uuid.uuid4())
        self.ts = utcnow_iso() if not self.ts else self.ts

    @property
    def kind_value(self) -> str:
        return self.kind.value if isinstance(self.kind, EventKind) else str(self.kind)

    def to_dict(self) -> Dict[str, Any]:
        payload = asdict(self)
        payload["kind"] = self.kind_value
        payload["id"] = self.id
        return payload

    @staticmethod
    def parse(data: Dict[str, Any]) -> "ContextEntry":
        return ContextEntry(
            project_id=data["project_id"],
            session_name=data["session_name"],
            actor=data["actor"],
            kind=data["kind"],
            text=data["text"],
            ts=data.get("ts", utcnow_iso()),
            refs=data.get("refs", {}),
            human_attention_needed=data.get("human_attention_needed", False),
            id=data.get("id"),
        )


@dataclass
class ProjectSnapshot:
    project_id: str
    summary: str
    status: str
    open_blockers: List[str] = field(default_factory=list)
    next_steps: List[str] = field(default_factory=list)
    recent_highlights: List[str] = field(default_factory=list)
    updated_at: str = field(default_factory=utcnow_iso)

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    @staticmethod
    def parse(data: Dict[str, Any]) -> "ProjectSnapshot":
        return ProjectSnapshot(
            project_id=data["project_id"],
            summary=data.get("summary", ""),
            status=data.get("status", "active"),
            open_blockers=data.get("open_blockers", []),
            next_steps=data.get("next_steps", []),
            recent_highlights=data.get("recent_highlights", []),
            updated_at=data.get("updated_at", utcnow_iso()),
        )
