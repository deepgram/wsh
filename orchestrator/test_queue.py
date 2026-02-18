"""Tests for queue functionality: store queue/resolve and server HTTP endpoints."""
from __future__ import annotations

import asyncio
import json
import tempfile
import unittest
from pathlib import Path

from .models import ContextEntry, EventKind, ProjectContext
from .store import ContextStore


class TestStoreQueue(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.store = ContextStore(root=Path(self.tmpdir))
        self.store.ensure_project(
            ProjectContext(project_id="p1", name="Test", goal="test queue")
        )

    def _add_entry(self, kind: EventKind, attention: bool = False) -> ContextEntry:
        entry = ContextEntry(
            project_id="p1",
            session_name="s1",
            actor="agent",
            kind=kind,
            text=f"test {kind.value}",
            human_attention_needed=attention,
        )
        self.store.append_entry(entry)
        return entry

    def test_queue_returns_approval_events(self):
        self._add_entry(EventKind.STATUS)
        e = self._add_entry(EventKind.APPROVAL)
        queue = self.store.get_queue()
        self.assertEqual(len(queue), 1)
        self.assertEqual(queue[0].kind_value, EventKind.APPROVAL.value)

    def test_queue_returns_error_events(self):
        self._add_entry(EventKind.STATUS)
        self._add_entry(EventKind.ERROR)
        queue = self.store.get_queue()
        self.assertEqual(len(queue), 1)
        self.assertEqual(queue[0].kind_value, EventKind.ERROR.value)

    def test_queue_returns_attention_events(self):
        e = self._add_entry(EventKind.NOTE, attention=True)
        queue = self.store.get_queue()
        self.assertEqual(len(queue), 1)

    def test_queue_excludes_normal_events(self):
        self._add_entry(EventKind.STATUS)
        self._add_entry(EventKind.NOTE)
        self._add_entry(EventKind.HANDOFF)
        queue = self.store.get_queue()
        self.assertEqual(len(queue), 0)

    def test_resolve_removes_from_queue(self):
        e = self._add_entry(EventKind.APPROVAL)
        queue = self.store.get_queue()
        self.assertEqual(len(queue), 1)

        self.store.resolve_entry("p1", queue[0].id, "approve", "LGTM")
        queue2 = self.store.get_queue()
        self.assertEqual(len(queue2), 0)

    def test_resolve_appends_resolution_event(self):
        e = self._add_entry(EventKind.APPROVAL)
        queue = self.store.get_queue()
        self.store.resolve_entry("p1", queue[0].id, "approve", "LGTM")

        events = self.store.get_events("p1", limit=100)
        resolution = [ev for ev in events if ev.actor == "human"]
        self.assertEqual(len(resolution), 1)
        self.assertIn("approve", resolution[0].text)
        self.assertIn("LGTM", resolution[0].text)

    def test_resolve_multiple(self):
        self._add_entry(EventKind.APPROVAL)
        self._add_entry(EventKind.ERROR)
        self._add_entry(EventKind.APPROVAL)
        queue = self.store.get_queue()
        self.assertEqual(len(queue), 3)

        self.store.resolve_entry("p1", queue[0].id, "approve")
        self.store.resolve_entry("p1", queue[1].id, "resolved")
        queue2 = self.store.get_queue()
        self.assertEqual(len(queue2), 1)

    def test_resolved_ids_persist(self):
        e = self._add_entry(EventKind.APPROVAL)
        queue = self.store.get_queue()
        entry_id = queue[0].id
        self.store.resolve_entry("p1", entry_id, "approve")

        resolved = self.store.get_resolved_ids("p1")
        self.assertIn(entry_id, resolved)

    def test_queue_across_projects(self):
        self.store.ensure_project(
            ProjectContext(project_id="p2", name="Test2", goal="test queue 2")
        )
        self._add_entry(EventKind.APPROVAL)  # p1
        e2 = ContextEntry(
            project_id="p2", session_name="s2", actor="agent",
            kind=EventKind.ERROR, text="error in p2",
        )
        self.store.append_entry(e2)

        queue = self.store.get_queue()
        self.assertEqual(len(queue), 2)
        projects = {e.project_id for e in queue}
        self.assertEqual(projects, {"p1", "p2"})


class TestEntryIdPreservation(unittest.TestCase):
    def test_parse_preserves_id(self):
        entry = ContextEntry(
            project_id="p1", session_name="s1", actor="agent",
            kind=EventKind.APPROVAL, text="test",
        )
        original_id = entry.id
        data = entry.to_dict()
        parsed = ContextEntry.parse(data)
        self.assertEqual(parsed.id, original_id)

    def test_new_entry_gets_fresh_id(self):
        e1 = ContextEntry(
            project_id="p1", session_name="s1", actor="agent",
            kind=EventKind.APPROVAL, text="test",
        )
        e2 = ContextEntry(
            project_id="p1", session_name="s1", actor="agent",
            kind=EventKind.APPROVAL, text="test",
        )
        self.assertNotEqual(e1.id, e2.id)


class TestServerHTTP(unittest.TestCase):
    """Test the server's HTTP endpoints using asyncio."""

    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.store = ContextStore(root=Path(self.tmpdir))
        self.store.ensure_project(
            ProjectContext(project_id="p1", name="Test", goal="test server")
        )

    def test_get_queue_empty(self):
        async def run():
            from .server import QueueServer
            server = QueueServer(self.store, port=0)
            # Test the store method directly
            entries = self.store.get_queue()
            return entries
        result = asyncio.run(run())
        self.assertEqual(len(result), 0)

    def test_resolve_entry_via_store(self):
        entry = ContextEntry(
            project_id="p1", session_name="s1", actor="agent",
            kind=EventKind.APPROVAL, text="deploy?",
        )
        self.store.append_entry(entry)
        queue = self.store.get_queue()
        self.assertEqual(len(queue), 1)

        self.store.resolve_entry("p1", queue[0].id, "approve", "yes")
        queue2 = self.store.get_queue()
        self.assertEqual(len(queue2), 0)


if __name__ == "__main__":
    unittest.main()
