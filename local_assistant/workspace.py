"""Workspace-scoped file tools and approval-gated local commands."""

from __future__ import annotations

import difflib
import fnmatch
import os
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path
from typing import Any, Callable

from .runtime import _terminal_safe


class ToolError(RuntimeError):
    """An invalid or unsafe tool request."""


EXCLUDED_DIRS = {
    ".git", ".hg", ".svn", ".venv", "venv", "node_modules", "__pycache__",
    ".mypy_cache", ".pytest_cache", ".ruff_cache", ".next", ".nuxt", "target",
    "dist", "build", ".idea", ".vscode",
}
SENSITIVE_NAMES = {
    ".env", "credentials.json", "credentials.yaml", "credentials.yml", "secrets.json",
    "secrets.yaml", "secrets.yml", "id_rsa", "id_ed25519", "known_hosts",
}
SENSITIVE_SUFFIXES = {".pem", ".key", ".pfx", ".p12", ".keystore"}
IGNORED_SUFFIXES = {
    ".gguf", ".safetensors", ".pt", ".pth", ".bin", ".onnx", ".sqlite", ".db",
    ".dll", ".exe", ".pyd", ".pyc", ".zip", ".7z", ".pdf", ".png", ".jpg",
    ".jpeg", ".webp", ".mp4", ".mp3", ".wav",
}
MAX_READ_BYTES = 2 * 1024 * 1024
MAX_WRITE_CHARS = 150_000


TOOL_SCHEMAS: list[dict[str, Any]] = [
    {
        "type": "function",
        "function": {
            "name": "list_files",
            "description": "List source and project files under a workspace folder. Ignores common generated folders and common .gitignore patterns; skips secrets and binary assets.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Workspace-relative folder, usually '.'"},
                    "max_depth": {"type": "integer", "minimum": 1, "maximum": 6},
                    "max_entries": {"type": "integer", "minimum": 1, "maximum": 200},
                },
                "required": ["path"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "read_file",
            "description": "Read a text file inside the workspace, optionally by line range. Secret files, binary files, and files over 2 MiB are blocked.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "start_line": {"type": "integer", "minimum": 1},
                    "end_line": {"type": "integer", "minimum": 1},
                },
                "required": ["path"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "search_text",
            "description": "Search for a literal, case-insensitive string in workspace text files. This is not a regular expression search.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "path": {"type": "string", "description": "Workspace-relative folder or file; defaults to '.'"},
                    "file_glob": {"type": "string", "description": "Filename glob such as '*.py' or '*'"},
                    "max_results": {"type": "integer", "minimum": 1, "maximum": 50},
                },
                "required": ["query"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "write_file",
            "description": "Create a new text file or overwrite an existing file in the workspace with UTF-8 content. Always call this tool directly when creating or updating files.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                },
                "required": ["path", "content"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "replace_text",
            "description": "Replace an exact, unique text block in a workspace UTF-8 text file. Always call this tool directly when editing files.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_text": {"type": "string"},
                    "new_text": {"type": "string"},
                },
                "required": ["path", "old_text", "new_text"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "delete_file",
            "description": "Delete a single file inside the workspace. Always call this tool directly when a file needs to be deleted. Directories cannot be deleted with this tool.",
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
                "additionalProperties": False,
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "run_command",
            "description": "Execute a PowerShell command in the workspace. Use this tool whenever you need to create directories, run scripts, execute terminal commands, install packages, or inspect system state. Always call this tool directly. Quote paths containing spaces.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The exact PowerShell command to execute (quote paths containing spaces)"},
                    "timeout_seconds": {"type": "integer", "minimum": 1, "maximum": 300},
                },
                "required": ["command"],
                "additionalProperties": False,
            },
        },
    },
]


class WorkspaceTools:
    PERMISSION_MODES = {"ask", "workspace", "full"}

    def __init__(
        self,
        workspace: Path,
        permission_mode: str = "ask",
        process_attacher: Callable[[subprocess.Popen[Any]], None] | None = None,
    ) -> None:
        self.root = workspace.expanduser().resolve(strict=True)
        if not self.root.is_dir():
            raise ToolError(f"Workspace is not a directory: {self.root}")
        self.permission_mode = "ask"
        self.set_permission_mode(permission_mode)
        self.process_attacher = process_attacher
        self.gitignore = self._read_gitignore()

    def set_permission_mode(self, mode: str) -> None:
        if mode not in self.PERMISSION_MODES:
            raise ToolError("Permission mode must be ask, workspace, or full.")
        self.permission_mode = mode

    def _authorize(self, prompt: str, *, scope: str) -> bool:
        if self.permission_mode == "full":
            print("\n[FULL ACCESS] Proceeding without per-action approval.")
            return True
        if self.permission_mode == "workspace" and scope == "workspace":
            print("\n[WORKSPACE MODE] Applying this workspace file change without per-action approval.")
            return True
        return _confirm(prompt)

    def dispatch(self, name: str, arguments: dict[str, Any]) -> str:
        handlers = {
            "list_files": self.list_files,
            "read_file": self.read_file,
            "search_text": self.search_text,
            "write_file": self.write_file,
            "replace_text": self.replace_text,
            "delete_file": self.delete_file,
            "run_command": self.run_command,
        }
        handler = handlers.get(name)
        if handler is None:
            raise ToolError(f"Unknown tool: {name}")
        return handler(**arguments)

    def _safe_path(self, relative: str, *, allow_root: bool = False) -> Path:
        if not isinstance(relative, str) or not relative.strip():
            if allow_root:
                return self.root
            raise ToolError("A non-empty workspace-relative path is required.")
        requested = Path(relative)
        if requested.is_absolute() or requested.drive:
            raise ToolError("Absolute paths are not allowed; use a workspace-relative path.")
        if os.name == "nt" and any(":" in part for part in requested.parts):
            raise ToolError("Windows alternate data stream paths are blocked.")
        target = (self.root / requested).resolve(strict=False)
        try:
            target.relative_to(self.root)
        except ValueError as exc:
            raise ToolError("That path resolves outside the selected workspace.") from exc

        cursor = self.root
        for part in requested.parts:
            if part in ("", "."):
                continue
            cursor = cursor / part
            if cursor.exists() and _is_reparse_point(cursor):
                raise ToolError("Symlinks and Windows junctions are not followed by workspace tools.")
        return target

    def _read_gitignore(self) -> list[str]:
        file = self.root / ".gitignore"
        if not file.is_file() or _is_reparse_point(file):
            return []
        try:
            if file.stat().st_size > 100_000:
                return []
            return [
                line.strip()
                for line in file.read_text(encoding="utf-8-sig").splitlines()
                if line.strip() and not line.lstrip().startswith("#") and not line.startswith("!")
            ]
        except (OSError, UnicodeError):
            return []

    def _ignored(self, path: Path) -> bool:
        try:
            relative = path.relative_to(self.root).as_posix()
        except ValueError:
            return True
        parts = relative.split("/")
        if any(part.lower() in EXCLUDED_DIRS for part in parts[:-1]):
            return True
        if _is_sensitive(path, parts):
            return True
        if path.suffix.lower() in IGNORED_SUFFIXES:
            return True
        for pattern in self.gitignore:
            normalized = pattern.replace("\\", "/").rstrip("/")
            if not normalized:
                continue
            if "/" in normalized:
                if fnmatch.fnmatchcase(relative, normalized) or relative.startswith(normalized + "/"):
                    return True
            elif any(fnmatch.fnmatchcase(part, normalized) for part in parts):
                return True
        return False

    def _walk_files(self, base: Path, max_depth: int = 8):
        start_depth = len(base.relative_to(self.root).parts)
        for current, directories, filenames in os.walk(base, followlinks=False):
            folder = Path(current)
            depth = len(folder.relative_to(self.root).parts) - start_depth
            directories[:] = [
                name for name in directories
                if name.lower() not in EXCLUDED_DIRS
                and not self._ignored(folder / name)
                and not _is_reparse_point(folder / name)
            ]
            if depth >= max_depth:
                directories.clear()
            for filename in filenames:
                path = folder / filename
                if not self._ignored(path) and not _is_reparse_point(path):
                    yield path

    def list_files(
        self,
        path: str = ".",
        max_depth: int = 4,
        max_entries: int = 120,
    ) -> str:
        max_depth = _bounded_int(max_depth, 1, 6, "max_depth")
        max_entries = _bounded_int(max_entries, 1, 200, "max_entries")
        folder = self._safe_path(path, allow_root=True)
        if not folder.is_dir():
            raise ToolError(f"Not a workspace folder: {path}")
        if self._ignored(folder):
            raise ToolError("This folder is excluded by the workspace file policy.")

        entries: list[str] = []
        for file in self._walk_files(folder, max_depth=max_depth):
            try:
                size = file.stat().st_size
            except OSError:
                continue
            relative = file.relative_to(self.root).as_posix()
            suffix = " [large file; use a focused export]" if size > MAX_READ_BYTES else ""
            entries.append(f"{relative} ({size} bytes){suffix}")
            if len(entries) >= max_entries:
                break
        if not entries:
            return "No readable source files found. Common generated folders, Git-ignored files, secrets, and binary assets are omitted."
        return "\n".join(entries)

    def read_file(
        self,
        path: str,
        start_line: int = 1,
        end_line: int | None = None,
    ) -> str:
        file = self._safe_path(path)
        if self._ignored(file):
            raise ToolError("This file is ignored, binary, or marked as sensitive by the workspace policy.")
        if not file.is_file():
            raise ToolError(f"Not a workspace file: {path}")
        if file.stat().st_size > MAX_READ_BYTES:
            raise ToolError("File is over 2 MiB; inspect a smaller source file instead.")
        data = file.read_bytes()
        if b"\0" in data:
            raise ToolError("Binary files cannot be read by this tool.")
        try:
            content = data.decode("utf-8-sig")
        except UnicodeDecodeError as exc:
            raise ToolError("File is not UTF-8 text and was not read.") from exc
        lines = content.splitlines()
        start = _bounded_int(start_line, 1, max(1, len(lines)), "start_line")
        end = len(lines) if end_line is None else _bounded_int(end_line, start, max(start, len(lines)), "end_line")
        if end - start + 1 > 400:
            end = start + 399
        body = "\n".join(f"{index}: {lines[index - 1]}" for index in range(start, min(end, len(lines)) + 1))
        return f"File: {file.relative_to(self.root).as_posix()} (lines {start}-{min(end, len(lines))})\n{body}"

    def search_text(
        self,
        query: str,
        path: str = ".",
        file_glob: str = "*",
        max_results: int = 30,
    ) -> str:
        if not isinstance(query, str) or not query.strip():
            raise ToolError("Search query cannot be empty.")
        if len(query) > 500:
            raise ToolError("Search query is too long.")
        if not isinstance(file_glob, str) or len(file_glob) > 120:
            raise ToolError("Filename glob is invalid.")
        max_results = _bounded_int(max_results, 1, 50, "max_results")
        base = self._safe_path(path, allow_root=True)
        if not base.exists():
            raise ToolError(f"Workspace path does not exist: {path}")
        if self._ignored(base):
            raise ToolError("This path is excluded by the workspace file policy.")

        matches: list[str] = []
        files_scanned = 0
        iterable = [base] if base.is_file() else self._walk_files(base, max_depth=10)
        for file in iterable:
            if not file.is_file() or self._ignored(file) or not fnmatch.fnmatch(file.name, file_glob):
                continue
            files_scanned += 1
            if files_scanned > 4000:
                break
            try:
                if file.stat().st_size > MAX_READ_BYTES:
                    continue
                data = file.read_bytes()
                if b"\0" in data:
                    continue
                content = data.decode("utf-8-sig")
            except (OSError, UnicodeError):
                continue
            for line_number, line in enumerate(content.splitlines(), 1):
                if query.casefold() in line.casefold():
                    relative = file.relative_to(self.root).as_posix()
                    matches.append(f"{relative}:{line_number}: {line.strip()[:300]}")
                    if len(matches) >= max_results:
                        return "\n".join(matches) + "\n[results capped]"
        if matches:
            return "\n".join(matches)
        note = " Search stopped after 4,000 files." if files_scanned > 4000 else ""
        return f"No matches found in {min(files_scanned, 4000)} readable files.{note}"

    def write_file(self, path: str, content: str) -> str:
        if not isinstance(content, str) or len(content) > MAX_WRITE_CHARS:
            raise ToolError(f"File content must be text of at most {MAX_WRITE_CHARS:,} characters.")
        target = self._safe_path(path)
        if self._ignored(target):
            raise ToolError("This path is ignored, binary, or marked as sensitive by the workspace policy.")
        if target.exists() and not target.is_file():
            raise ToolError("write_file only creates or replaces files; it cannot replace a directory.")
        old_bytes = target.read_bytes() if target.is_file() else None
        if old_bytes is not None and len(old_bytes) > MAX_READ_BYTES:
            raise ToolError("Replacing files over 2 MiB is blocked. Use a focused text replacement instead.")
        old_text = _decode_text(old_bytes) if old_bytes is not None else ""
        proposed = _match_newlines(content, old_bytes)
        return self._approve_and_write(target, old_bytes, proposed)

    def replace_text(self, path: str, old_text: str, new_text: str) -> str:
        if not old_text:
            raise ToolError("old_text cannot be empty.")
        if len(old_text) > MAX_WRITE_CHARS or len(new_text) > MAX_WRITE_CHARS:
            raise ToolError("Replacement text is too long.")
        target = self._safe_path(path)
        if self._ignored(target):
            raise ToolError("This file is ignored, binary, or marked as sensitive by the workspace policy.")
        if not target.is_file() or target.stat().st_size > MAX_READ_BYTES:
            raise ToolError("replace_text requires a readable text file no larger than 2 MiB.")
        old_bytes = target.read_bytes()
        original = _decode_text(old_bytes)
        normalized_original = original.replace("\r\n", "\n").replace("\r", "\n")
        normalized_old = old_text.replace("\r\n", "\n").replace("\r", "\n")
        normalized_new = new_text.replace("\r\n", "\n").replace("\r", "\n")
        occurrences = normalized_original.count(normalized_old)
        if occurrences != 1:
            raise ToolError(f"Expected old_text exactly once; found it {occurrences} times. Read the file and refine the selection.")
        changed = normalized_original.replace(normalized_old, normalized_new, 1)
        proposed = _match_newlines(changed, old_bytes)
        return self._approve_and_write(target, old_bytes, proposed)

    def _approve_and_write(self, target: Path, old_bytes: bytes | None, proposed: str) -> str:
        old_text = _decode_text(old_bytes) if old_bytes is not None else ""
        if proposed == old_text and old_bytes is not None:
            return f"No change needed: {target.relative_to(self.root).as_posix()}"
        relative = target.relative_to(self.root).as_posix()
        if not self._show_diff(relative, old_text, proposed):
            raise ToolError("The proposed diff is too large to review in the terminal; no file was changed.")
        if not self._authorize(f"Apply this change to {relative}? Type YES to approve", scope="workspace"):
            return f"User declined the write to {relative}. No file was changed."

        target = self._safe_path(relative)
        if old_bytes is None:
            if target.exists():
                raise ToolError("The file was created while the proposed change was awaiting approval; refusing to overwrite it.")
        else:
            if not target.is_file() or target.read_bytes() != old_bytes:
                raise ToolError("The file changed while the proposed diff was awaiting approval; reread it and propose again.")

        target.parent.mkdir(parents=True, exist_ok=True)
        self._safe_path(relative)
        payload = proposed.encode("utf-8")
        if old_bytes is not None and old_bytes.startswith(b"\xef\xbb\xbf") and not payload.startswith(b"\xef\xbb\xbf"):
            payload = b"\xef\xbb\xbf" + payload
        temporary: str | None = None
        try:
            with tempfile.NamedTemporaryFile("wb", dir=target.parent, delete=False) as stream:
                temporary = stream.name
                stream.write(payload)
            os.replace(temporary, target)
        finally:
            if temporary and os.path.exists(temporary):
                os.unlink(temporary)
        return f"Wrote {relative} ({len(payload)} bytes)."

    def _show_diff(self, relative: str, before: str, after: str) -> bool:
        diff = difflib.unified_diff(
            before.splitlines(keepends=True),
            after.splitlines(keepends=True),
            fromfile=f"a/{relative}" if before else "/dev/null",
            tofile=f"b/{relative}",
            n=3,
        )
        text = _terminal_safe("".join(diff))
        print("\nProposed change:\n")
        if len(text) > 50_000:
            print("Diff is over 50,000 characters; refusing to apply a change that cannot be fully reviewed.")
            return False
        print(text or "(No textual diff.)")
        return True

    def delete_file(self, path: str) -> str:
        target = self._safe_path(path)
        if self._ignored(target):
            raise ToolError("Deletion of ignored, binary, or sensitive files is blocked by the workspace policy.")
        if not target.is_file():
            raise ToolError("delete_file only deletes a single regular file.")
        before = target.read_bytes()
        if len(before) > MAX_READ_BYTES:
            raise ToolError("Deletion of files over 2 MiB is blocked.")
        content = _decode_text(before)
        if not self._show_diff(target.relative_to(self.root).as_posix(), content, ""):
            raise ToolError("The proposed deletion diff is too large to review; no file was changed.")
        relative = target.relative_to(self.root).as_posix()
        if not self._authorize(f"Delete {relative} ({len(before)} bytes)? Type YES to approve", scope="workspace"):
            return f"User declined deletion of {relative}. No file was changed."
        target = self._safe_path(relative)
        if not target.is_file() or target.read_bytes() != before:
            raise ToolError("The file changed while deletion was awaiting approval; refusing to remove it.")
        target.unlink()
        return f"Deleted {relative}."

    def run_command(self, command: str, timeout_seconds: int = 120) -> str:
        if not isinstance(command, str) or not command.strip():
            raise ToolError("Command cannot be empty.")
        if len(command) > 5000:
            raise ToolError("Command is too long to review safely.")
        timeout_seconds = _bounded_int(timeout_seconds, 1, 300, "timeout_seconds")
        shell = shutil.which("pwsh.exe") or shutil.which("powershell.exe")
        if not shell:
            raise ToolError("PowerShell was not found on PATH.")
        print("\nProposed PowerShell command:\n")
        print(_terminal_safe(command))
        print(f"\nWorking directory: {self.root}")
        print("This runs with your Windows account's permissions, outside an OS sandbox.")
        if not self._authorize("Run this command? Type YES to approve", scope="command"):
            return "User declined to run the command. No command was executed."

        args = [shell, "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", command]
        process: subprocess.Popen[str] | None = None
        try:
            process = subprocess.Popen(
                args,
                cwd=str(self.root),
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
            if self.process_attacher is not None:
                try:
                    self.process_attacher(process)
                except OSError as exc:
                    try:
                        process.kill()
                        process.wait(timeout=5)
                    except (OSError, subprocess.TimeoutExpired):
                        pass
                    raise ToolError(f"Could not attach the command to automatic session cleanup: {exc}") from exc
            stdout, _ = process.communicate(timeout=timeout_seconds)
            output = _terminal_safe(stdout or "")
            if len(output) > 16_000:
                output = output[:16_000] + "\n[command output capped]"
            return f"Exit code: {process.returncode}\n{output or '(no output)'}"
        except subprocess.TimeoutExpired:
            if process is not None:
                try:
                    process.kill()
                except OSError:
                    pass
                output, _ = process.communicate()
            else:
                output = ""
            return f"Command timed out after {timeout_seconds} seconds. The PowerShell process was stopped.\n{_terminal_safe((output or '')[-8_000:])}"
        except OSError as exc:
            raise ToolError(f"Could not start the approved command: {exc}") from exc


def _bounded_int(value: Any, low: int, high: int, name: str) -> int:
    if isinstance(value, bool):
        raise ToolError(f"{name} must be an integer.")
    try:
        converted = int(value)
    except (TypeError, ValueError) as exc:
        raise ToolError(f"{name} must be an integer.") from exc
    return max(low, min(high, converted))


def _is_reparse_point(path: Path) -> bool:
    try:
        info = path.lstat()
    except OSError:
        return False
    attributes = getattr(info, "st_file_attributes", 0)
    return path.is_symlink() or bool(attributes & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400))


def _is_sensitive(path: Path, relative_parts: list[str]) -> bool:
    name = path.name.lower()
    if name in SENSITIVE_NAMES or name.startswith(".env"):
        return True
    if path.suffix.lower() in SENSITIVE_SUFFIXES:
        return True
    if any(word in name for word in ("password", "secret", "credential")):
        return True
    return any(part.lower() in {".ssh", "secrets", "credentials"} for part in relative_parts[:-1])


def _decode_text(data: bytes | None) -> str:
    if data is None:
        return ""
    if b"\0" in data:
        raise ToolError("Binary files cannot be edited by this tool.")
    try:
        return data.decode("utf-8-sig")
    except UnicodeDecodeError as exc:
        raise ToolError("Only UTF-8 text files can be edited by this tool.") from exc


def _match_newlines(proposed: str, old_bytes: bytes | None) -> str:
    normalized = proposed.replace("\r\n", "\n").replace("\r", "\n")
    if old_bytes is not None and b"\r\n" in old_bytes:
        newline = "\r\n"
    elif old_bytes is None and os.name == "nt":
        newline = "\r\n"
    else:
        newline = "\n"
    return normalized.replace("\n", newline)


def _confirm(prompt: str) -> bool:
    try:
        answer = input(f"\n{prompt}: ").strip()
    except (EOFError, KeyboardInterrupt):
        print("\nApproval prompt cancelled.")
        return False
    return answer == "YES"
