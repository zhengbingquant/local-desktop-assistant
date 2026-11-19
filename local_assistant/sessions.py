"""Local chat transcripts and per-workspace resume selections."""

from __future__ import annotations

import hashlib
import json
import os
import re
import sqlite3
import stat
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import Any

from .runtime import _terminal_safe


def sessions_dir() -> Path:
    base = os.environ.get("LOCALAPPDATA")
    root = Path(base) / "LocalAssistant" if base else Path.home() / ".local" / "state" / "local-assistant"
    return root / "sessions"


def workspace_key(workspace: Path) -> str:
    return os.path.normcase(str(workspace))


def _preview(messages: list[dict[str, Any]]) -> str:
    for message in messages:
        if isinstance(message, dict) and message.get("role") == "user" and message.get("content"):
            return " ".join(str(message["content"])[:200].split())[:80]
    return ""


_LEGACY_NAME = re.compile(r"[a-f0-9]{16}(?:_[0-9]{8}_[0-9]{6})?\.json")


def _linked(path: Path) -> bool:
    info = path.lstat()
    return stat.S_ISLNK(info.st_mode) or bool(getattr(info, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT)


class SessionStore:
    def __init__(self) -> None:
        self.directory = sessions_dir()
        self.directory.mkdir(parents=True, exist_ok=True)
        self.db = sqlite3.connect(self.directory / "history.sqlite3")
        self.db.row_factory = sqlite3.Row
        self.db.execute("PRAGMA secure_delete = ON")
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS chats (
                id TEXT PRIMARY KEY, workspace_key TEXT NOT NULL, workspace TEXT NOT NULL,
                saved_ns INTEGER NOT NULL, revision INTEGER NOT NULL, preview TEXT NOT NULL,
                message_count INTEGER NOT NULL, model TEXT NOT NULL, messages TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS chats_by_time ON chats(saved_ns DESC, id);
            CREATE INDEX IF NOT EXISTS chats_by_workspace ON chats(workspace_key, saved_ns DESC, id);
            CREATE TABLE IF NOT EXISTS active (workspace_key TEXT PRIMARY KEY, chat_id TEXT);
        """)

    def list(self) -> list[sqlite3.Row]:
        return self.db.execute("""
            SELECT id, workspace, saved_ns, preview, message_count FROM chats ORDER BY saved_ns DESC, id
        """).fetchall()

    def current(self, workspace: Path, *, latest_if_fresh: bool = False) -> str | None:
        key = workspace_key(workspace)
        row = self.db.execute("SELECT chat_id FROM active WHERE workspace_key = ?", (key,)).fetchone()
        if row and (row[0] or not latest_if_fresh):
            return row[0]
        row = self.db.execute(
            "SELECT id FROM chats WHERE workspace_key = ? ORDER BY saved_ns DESC, id LIMIT 1", (key,)
        ).fetchone()
        return row[0] if row else None

    def get(self, chat_id: str) -> sqlite3.Row | None:
        return self.db.execute("SELECT * FROM chats WHERE id = ?", (chat_id,)).fetchone()

    def select(self, workspace: Path, chat_id: str | None) -> None:
        with self.db:
            if chat_id is None:
                self._set_active(workspace, None)
                return
            selected = self.db.execute("""
                INSERT OR REPLACE INTO active SELECT workspace_key, id FROM chats WHERE id=? AND workspace_key=?
            """, (chat_id, workspace_key(workspace)))
            if not selected.rowcount:
                raise ValueError("That conversation is no longer available.")

    def _set_active(self, workspace: Path, chat_id: str | None) -> None:
        self.db.execute("INSERT OR REPLACE INTO active VALUES (?, ?)", (workspace_key(workspace), chat_id))

    def save(
        self, workspace: Path, chat_id: str | None, revision: int, messages: list[dict[str, Any]],
        *, model: str, restore_deleted: bool = False,
    ) -> tuple[str, int]:
        body = json.dumps(messages, ensure_ascii=True, separators=(",", ":"))
        fields = (time.time_ns(), len(messages), model, body)
        with self.db:
            if chat_id is not None:
                updated = self.db.execute("""
                    UPDATE chats SET saved_ns=?, message_count=?, model=?, messages=?, revision=revision+1
                    WHERE id=? AND revision=?
                """, (*fields, chat_id, revision))
                if updated.rowcount:
                    self._set_active(workspace, chat_id)
                    return chat_id, revision + 1
                if not restore_deleted and not self.db.execute("SELECT 1 FROM chats WHERE id=?", (chat_id,)).fetchone():
                    raise ValueError("This chat was deleted in another window. Use /save to keep a new copy.")
            # Concurrent edits to the same revision become independent conversations.
            chat_id = uuid.uuid4().hex
            self.db.execute("""
                INSERT INTO chats (id, workspace_key, workspace, revision, preview, saved_ns, message_count, model, messages)
                VALUES (?, ?, ?, 1, ?, ?, ?, ?, ?)
            """, (chat_id, workspace_key(workspace), str(workspace), _preview(messages), *fields))
            self._set_active(workspace, chat_id)
        return chat_id, 1

    def import_profile(self) -> None:
        for path in self.directory.glob("*.json"):
            if _LEGACY_NAME.fullmatch(path.name):
                self._import_file(path)

    def import_workspace(self, workspace: Path) -> None:
        folder = workspace / ".local_assistant"
        path = folder / "chat-session.json"
        if path.is_file() and not _linked(folder):
            self._import_file(path, workspace)

    def _import_file(self, path: Path, workspace: Path | None = None) -> None:
        try:
            if _linked(path):
                return
            before = path.stat()
            raw = path.read_bytes()
            data = json.loads(raw.decode("utf-8-sig"))
            if not isinstance(data, dict) or not isinstance(data.get("messages"), list):
                raise ValueError("invalid transcript")
            legacy_workspace = workspace is not None
            if workspace is None:
                workspace = Path(data["workspace"])
                if not workspace.is_absolute():
                    raise ValueError("missing absolute workspace path")
            messages = data["messages"]
            if not any(isinstance(m, dict) and m.get("role") == "user" for m in messages):
                raise ValueError("no user messages in transcript")
            saved_ns = before.st_mtime_ns
            try:
                stamp = int(datetime.fromisoformat(data["saved_at"]).timestamp() * 1_000_000_000)
                if 0 <= stamp < 2**63:
                    saved_ns = stamp
            except (KeyError, TypeError, ValueError, OverflowError, OSError):
                pass
            # Include the contents so retrying an interrupted import is idempotent, and a
            # changed transcript from an older running binary gets a separate conversation.
            chat_id = hashlib.sha256(os.fsencode(path) + b"\0" + raw).hexdigest()
            key = workspace_key(workspace)
            body = json.dumps(messages, separators=(",", ":"))
            with self.db:
                self.db.execute("BEGIN IMMEDIATE")
                # A second window may have migrated or deleted this source while
                # it was being decoded. Do not recreate that chat from stale bytes.
                latest = path.stat()
                if (before.st_ino, before.st_mtime_ns, before.st_size) != (latest.st_ino, latest.st_mtime_ns, latest.st_size):
                    return
                self.db.execute("""
                    INSERT OR IGNORE INTO chats VALUES (?, ?, ?, ?, 1, ?, ?, ?, ?)
                """, (chat_id, key, str(workspace), saved_ns, _preview(messages), len(messages), str(data.get("model") or ""), body))
                current_name = hashlib.sha256(str(workspace).casefold().encode()).hexdigest()[:16] + ".json"
                if path.name == current_name or legacy_workspace:
                    self.db.execute("INSERT OR IGNORE INTO active VALUES (?, ?)", (key, chat_id))
            # A crash before unlink leaves the original for the next import. Remove only
            # the version we actually copied, after the database transaction has committed.
            after = path.stat()
            if (before.st_ino, before.st_mtime_ns, before.st_size) == (after.st_ino, after.st_mtime_ns, after.st_size):
                path.unlink()
        except FileNotFoundError:
            pass  # Another instance finished importing it.
        except (OSError, ValueError, KeyError, TypeError) as exc:
            print(_terminal_safe(f"Could not migrate {path!s} ({exc}). Any remaining JSON is available for recovery."))

    def delete(self, workspace: Path, *, all_workspaces: bool) -> int:
        key = workspace_key(workspace)
        # Include incomplete saves and JSON files that could not be imported. Only
        # touch names used by older releases and the current project's legacy file.
        prefix = hashlib.sha256(str(workspace).casefold().encode()).hexdigest()[:16]
        for path in self.directory.iterdir():
            if path.suffix not in {".json", ".tmp"} or not _LEGACY_NAME.fullmatch(path.with_suffix(".json").name):
                continue
            if all_workspaces or path.name.startswith(prefix):
                path.unlink(missing_ok=True)
        folder = workspace / ".local_assistant"
        if folder.is_dir() and not _linked(folder):
            (folder / "chat-session.json").unlink(missing_ok=True)
        with self.db:
            if all_workspaces:
                self.db.execute("DELETE FROM active")
                return self.db.execute("DELETE FROM chats").rowcount
            self.db.execute("DELETE FROM active WHERE workspace_key = ?", (key,))
            return self.db.execute("DELETE FROM chats WHERE workspace_key = ?", (key,)).rowcount
