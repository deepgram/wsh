#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import List, Optional

from .config import OrchestratorConfig
from .engine import Orchestrator
from .wsh_client import WshClientError


def add_common_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--wsh-base-url", default=None, help="WSH base URL (defaults to env WSH_ORCH_BASE_URL)")
    parser.add_argument("--token", default=None, help="Optional bearer token for WSH")
    parser.add_argument("--state-dir", default=None, help="Path for orchestrator context state")


def create_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Wsh external orchestrator")
    add_common_args(parser)

    sub = parser.add_subparsers(dest="command", required=True)

    init = sub.add_parser("init", help="initialize a project context")
    init.add_argument("project_id")
    init.add_argument("name")
    init.add_argument("goal")
    init.add_argument("--branch", default=None, help="initial branch hint")

    assign = sub.add_parser("assign", help="create a session and dispatch one or more commands")
    assign.add_argument("project_id")
    assign.add_argument("role")
    assign.add_argument("commands", nargs="+", help="Shell command(s) to run")
    assign.add_argument("--session-name", default=None, help="Optional fixed session name")
    assign.add_argument("--heartbeat", action="store_true", default=False, help="Emit heartbeat context updates")

    send = sub.add_parser("send", help="send a command to an existing tracked session")
    send.add_argument("project_id")
    send.add_argument("session_name")
    send.add_argument("command")

    status = sub.add_parser("status", help="show current snapshot for a project")
    status.add_argument("project_id")

    pull = sub.add_parser("pull", help="pull latest screen/scrollback for a session")
    pull.add_argument("project_id")
    pull.add_argument("session_name")

    list_sessions = sub.add_parser("list", help="list active sessions from wsh")
    return parser


def apply_config(args: argparse.Namespace) -> OrchestratorConfig:
    cfg = OrchestratorConfig.from_env()
    if args.wsh_base_url:
        cfg.wsh_base_url = args.wsh_base_url
    if args.token:
        cfg.token = args.token
    if args.state_dir:
        cfg.state_dir = Path(args.state_dir)
    return cfg


def main() -> int:
    parser = create_parser()
    args = parser.parse_args()
    orchestrator = Orchestrator(apply_config(args))

    try:
        if args.command == "init":
            project = orchestrator.ensure_project(args.project_id, args.name, args.goal, branch=args.branch)
            print(json.dumps(project.__dict__, indent=2))
            return 0

        if args.command == "assign":
            responses = orchestrator.run_task(
                project_id=args.project_id,
                role=args.role,
                commands=args.commands,
                session_name=args.session_name,
                heartbeat_interval=orchestrator.config.poll_interval_seconds if args.heartbeat else 0,
            )
            for index, response in enumerate(responses, 1):
                print(f"[{index}] {response}")
            return 0

        if args.command == "send":
            response = orchestrator.dispatch_command(
                project_id=args.project_id,
                session_name=args.session_name,
                command=args.command,
                heartbeat=True,
            )
            print(response)
            return 0

        if args.command == "status":
            report = orchestrator.project_report(args.project_id)
            print(json.dumps(report, indent=2))
            return 0

        if args.command == "pull":
            payload = orchestrator.pull_session(args.project_id, args.session_name)
            print(json.dumps(payload, indent=2))
            return 0

        if args.command == "list":
            sessions = orchestrator.list_wsh_sessions()
            print(json.dumps(sessions, indent=2))
            return 0

        parser.print_help()
        return 1
    except RuntimeError as exc:
        print(f"orchestrator runtime error: {exc}")
        return 2
    except WshClientError as exc:
        print(f"wsh API error: {exc}")
        return 3


if __name__ == "__main__":
    raise SystemExit(main())
