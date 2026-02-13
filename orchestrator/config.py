from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path


@dataclass
class OrchestratorConfig:
    wsh_base_url: str = "http://127.0.0.1:8080"
    token: str | None = None
    state_dir: Path = Path.home() / ".local" / "share" / "wsh-orchestrator"
    poll_interval_seconds: float = 60.0

    @classmethod
    def from_env(cls) -> "OrchestratorConfig":
        return cls(
            wsh_base_url=os.getenv("WSH_ORCH_BASE_URL", "http://127.0.0.1:8080"),
            token=os.getenv("WSH_ORCH_TOKEN"),
            state_dir=Path(os.getenv("WSH_ORCH_STATE_DIR", Path.home() / ".local" / "share" / "wsh-orchestrator")),
            poll_interval_seconds=float(os.getenv("WSH_ORCH_POLL_INTERVAL_SECONDS", "60.0")),
        )
