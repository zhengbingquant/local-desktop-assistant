"""Private llama.cpp server lifecycle and OpenAI-compatible chat transport."""

from __future__ import annotations

import json
import os
import secrets
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Callable

from .models import ModelInfo, RuntimeTuning


MAX_RESPONSE_TOKENS = 4096


class ModelError(RuntimeError):
    """Raised when the local inference process cannot serve a response."""


class ContextOverflowError(ModelError):
    """The request no longer fits in the model's context window."""

    def __init__(self, message: str, prompt_tokens: int = 0, context_tokens: int = 0) -> None:
        super().__init__(message)
        self.prompt_tokens = prompt_tokens
        self.context_tokens = context_tokens


class _WindowsKillOnCloseJob:
    """Tie the model server and its descendants to this process on Windows."""

    def __init__(self) -> None:
        import ctypes
        from ctypes import wintypes

        class BasicLimitInformation(ctypes.Structure):
            _fields_ = [
                ("PerProcessUserTimeLimit", ctypes.c_int64),
                ("PerJobUserTimeLimit", ctypes.c_int64),
                ("LimitFlags", wintypes.DWORD),
                ("MinimumWorkingSetSize", ctypes.c_size_t),
                ("MaximumWorkingSetSize", ctypes.c_size_t),
                ("ActiveProcessLimit", wintypes.DWORD),
                ("Affinity", ctypes.c_size_t),
                ("PriorityClass", wintypes.DWORD),
                ("SchedulingClass", wintypes.DWORD),
            ]

        class IoCounters(ctypes.Structure):
            _fields_ = [
                ("ReadOperationCount", ctypes.c_uint64),
                ("WriteOperationCount", ctypes.c_uint64),
                ("OtherOperationCount", ctypes.c_uint64),
                ("ReadTransferCount", ctypes.c_uint64),
                ("WriteTransferCount", ctypes.c_uint64),
                ("OtherTransferCount", ctypes.c_uint64),
            ]

        class ExtendedLimitInformation(ctypes.Structure):
            _fields_ = [
                ("BasicLimitInformation", BasicLimitInformation),
                ("IoInfo", IoCounters),
                ("ProcessMemoryLimit", ctypes.c_size_t),
                ("JobMemoryLimit", ctypes.c_size_t),
                ("PeakProcessMemoryUsed", ctypes.c_size_t),
                ("PeakJobMemoryUsed", ctypes.c_size_t),
            ]

        self._ctypes = ctypes
        self._kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        self._kernel32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        self._kernel32.CreateJobObjectW.restype = wintypes.HANDLE
        self._kernel32.SetInformationJobObject.argtypes = [
            wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD,
        ]
        self._kernel32.SetInformationJobObject.restype = wintypes.BOOL
        self._kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        self._kernel32.AssignProcessToJobObject.restype = wintypes.BOOL
        self._kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        self._kernel32.CloseHandle.restype = wintypes.BOOL
        self._handle = self._kernel32.CreateJobObjectW(None, None)
        if not self._handle:
            raise OSError(ctypes.get_last_error(), "CreateJobObjectW failed")

        limits = ExtendedLimitInformation()
        limits.BasicLimitInformation.LimitFlags = 0x00002000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not self._kernel32.SetInformationJobObject(
            self._handle,
            9,  # JobObjectExtendedLimitInformation
            ctypes.byref(limits),
            ctypes.sizeof(limits),
        ):
            error = ctypes.get_last_error()
            self.close()
            raise OSError(error, "SetInformationJobObject failed")

    def assign(self, process: subprocess.Popen[Any]) -> None:
        from ctypes import wintypes

        process_handle = getattr(process, "_handle", None)
        if process_handle is None:
            raise OSError("Could not obtain the model server process handle for cleanup.")
        if not self._kernel32.AssignProcessToJobObject(self._handle, wintypes.HANDLE(int(process_handle))):
            raise OSError(self._ctypes.get_last_error(), "AssignProcessToJobObject failed")

    def close(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle:
            self._kernel32.CloseHandle(handle)
            self._handle = None


class LocalModelServer:
    def __init__(
        self,
        server_exe: Path,
        model: ModelInfo,
        tuning: RuntimeTuning,
        *,
        reasoning_effort: str = "medium",
        thinking_budget_tokens: int = 512,
        draft_model_file: Path | None = None,
        startup_timeout: int = 900,
    ) -> None:
        self.server_exe = server_exe.resolve()
        self.info = model
        self.model_file = model.path
        self.tuning = tuning
        self.context_length = tuning.context_length
        self.reasoning_effort = reasoning_effort
        self.thinking_budget_tokens = thinking_budget_tokens
        self.draft_model_file = draft_model_file.resolve() if draft_model_file else None
        self.startup_timeout = startup_timeout
        self.process: subprocess.Popen[bytes] | None = None
        self.port: int | None = None
        self.api_key = secrets.token_urlsafe(32)
        self._temp_dir: tempfile.TemporaryDirectory[str] | None = None
        self._log_file: Path | None = None
        self._job: _WindowsKillOnCloseJob | None = None
        self._opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    @property
    def base_url(self) -> str:
        if self.port is None:
            raise ModelError("The local inference server has not started.")
        return f"http://127.0.0.1:{self.port}"

    def attach_process(self, process: subprocess.Popen[Any]) -> None:
        """Make another assistant-launched process part of this session's cleanup job."""
        if self._job is not None:
            self._job.assign(process)

    def command(self) -> list[str]:
        tuning = self.tuning
        command = [
            str(self.server_exe),
            "--model", str(self.model_file),
            "--host", "127.0.0.1",
            "--port", str(self.port),
            "--api-key", self.api_key,
            "--ctx-size", str(tuning.context_length),
            "--parallel", "1",
            "--fit", "on",
            "--fit-target", str(tuning.gpu_reserve_mib),
            "--flash-attn", "on",
            "--batch-size", str(tuning.batch_size),
            "--ubatch-size", str(tuning.ubatch_size),
            "--cache-type-k", tuning.kv_cache_type,
            "--cache-type-v", tuning.kv_cache_type,
            "--jinja",
            "--offline",
            "--no-webui",
            "--no-agent",
            "--reasoning", "on" if self.info.reasoning else "off",
        ]
        if tuning.cpu_threads is not None:
            command += ["--threads", str(tuning.cpu_threads)]
        if tuning.batch_threads is not None:
            command += ["--threads-batch", str(tuning.batch_threads)]
        if tuning.speculative != "none":
            command += ["--spec-type", tuning.speculative]
        if tuning.speculative == "draft-mtp":
            command += ["--spec-draft-n-max", str(tuning.draft_max)]
            if self.draft_model_file is not None:
                command += ["--model-draft", str(self.draft_model_file)]
        return command

    def __enter__(self) -> "LocalModelServer":
        if not self.server_exe.is_file():
            raise ModelError(
                f"llama-server is missing: {self.server_exe}\n"
                "Run .\\scripts\\install-runtime.ps1 to download llama.cpp, or place a build in runtime\\llama.cpp."
            )
        if not self.model_file.is_file():
            raise ModelError(
                f"The local model is missing: {self.model_file}\n"
                "Run .\\scripts\\download-model.ps1 to download it."
            )
        if self.draft_model_file is not None and not self.draft_model_file.is_file():
            raise ModelError(f"The draft model is missing: {self.draft_model_file}")

        self._temp_dir = tempfile.TemporaryDirectory(prefix="local-assistant-")
        self._log_file = Path(self._temp_dir.name) / "llama-server.log"
        self.port = self._free_local_port()
        command = self.command()

        try:
            if os.name == "nt":
                self._job = _WindowsKillOnCloseJob()
            with self._log_file.open("wb") as log:
                creation_flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
                self.process = subprocess.Popen(
                    command,
                    cwd=str(self.server_exe.parent),
                    stdin=subprocess.DEVNULL,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    creationflags=creation_flags,
                )
            if self._job is not None:
                self._job.assign(self.process)
        except OSError as exc:
            self.close()
            raise ModelError(f"Could not start llama-server with automatic process cleanup: {exc}") from exc

        print("Starting the local model. First load can take several minutes.")
        started = time.monotonic()
        last_notice = started
        try:
            while time.monotonic() - started < self.startup_timeout:
                if self.process.poll() is not None:
                    detail = self._read_server_log()
                    self.close()
                    raise ModelError(
                        "llama-server stopped while loading the model.\n"
                        + (detail or "No server log was produced.")
                    )
                try:
                    self._request("GET", "/health", timeout=2)
                    print(f"Local model is ready ({time.monotonic() - started:.0f}s).\n")
                    return self
                except ModelError as exc:
                    message = str(exc)
                    if "HTTP 503" not in message and "Could not reach the local model" not in message:
                        detail = self._read_server_log()
                        self.close()
                        raise ModelError(f"Local server health check failed: {message}\n{detail}") from exc

                now = time.monotonic()
                if now - last_notice >= 15:
                    print("  Still loading the model...")
                    last_notice = now
                time.sleep(0.25)
        except KeyboardInterrupt:
            self.close()
            raise

        detail = self._read_server_log()
        self.close()
        raise ModelError(
            f"Model startup timed out after {self.startup_timeout} seconds.\n"
            + (detail or "No server log was produced.")
        )

    @staticmethod
    def _free_local_port() -> int:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(("127.0.0.1", 0))
            return int(sock.getsockname()[1])

    def _request(
        self,
        method: str,
        route: str,
        body: dict[str, Any] | None = None,
        timeout: int = 30,
    ) -> bytes:
        data = json.dumps(body).encode("utf-8") if body is not None else None
        request = urllib.request.Request(
            self.base_url + route,
            data=data,
            method=method,
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
                "Accept": "application/json",
            },
        )
        try:
            with self._opener.open(request, timeout=timeout) as response:
                return response.read()
        except urllib.error.HTTPError as exc:
            detail = exc.read(16_000).decode("utf-8", errors="replace")
            raise ModelError(f"Local model returned HTTP {exc.code}: {detail}") from exc
        except (OSError, urllib.error.URLError) as exc:
            raise ModelError(f"Could not reach the local model: {exc}") from exc

    def stream_chat(
        self,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]],
        on_text: Callable[[str], None],
        on_thought: Callable[[str], None] | None = None,
        *,
        prompt_tokens: int | None = None,
        tool_choice: str = "auto",
    ) -> dict[str, Any]:
        # Leave the rest of the context window for the reply, capped at a sane maximum.
        room = self.context_length - (prompt_tokens or 0) - 64
        body: dict[str, Any] = {
            "model": self.model_file.stem,
            "messages": messages,
            "temperature": self.info.temperature,
            "top_p": self.info.top_p,
            "top_k": self.info.top_k,
            "max_tokens": max(256, min(MAX_RESPONSE_TOKENS, room)),
            "stream": True,
            "stream_options": {"include_usage": True},
        }
        if self.info.reasoning:
            if self.reasoning_effort == "none":
                # llama.cpp treats reasoning_effort "none" as unset, and Qwen templates then
                # default to their deepest effort. Disabling thinking must use the template switch.
                body["chat_template_kwargs"] = {"enable_thinking": False}
            else:
                body["reasoning_effort"] = self.reasoning_effort
                if self.thinking_budget_tokens > 0:
                    body["thinking_budget_tokens"] = self.thinking_budget_tokens
        if tools:
            body["tools"] = tools
            body["tool_choice"] = tool_choice
            body["parallel_tool_calls"] = False

        payload = json.dumps(body, ensure_ascii=False).encode("utf-8")
        request = urllib.request.Request(
            self.base_url + "/v1/chat/completions",
            data=payload,
            method="POST",
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
                "Accept": "text/event-stream",
            },
        )

        content: list[str] = []
        reasoning: list[str] = []
        tool_fragments: dict[int, dict[str, Any]] = {}
        response_id: str | None = None
        finish_reason: str | None = None
        usage: dict[str, Any] = {}
        timings: dict[str, Any] = {}
        status_visible = True
        print("Thinking...", end="", flush=True)

        def clear_status() -> None:
            nonlocal status_visible
            if status_visible:
                print("\r" + " " * 16 + "\r", end="", flush=True)
                status_visible = False

        try:
            with self._opener.open(request, timeout=3600) as response:
                for raw_line in response:
                    line = raw_line.decode("utf-8", errors="replace").strip()
                    if not line.startswith("data:"):
                        continue
                    data = line[5:].strip()
                    if not data or data == "[DONE]":
                        continue
                    try:
                        chunk = json.loads(data)
                    except json.JSONDecodeError as exc:
                        raise ModelError(f"Invalid stream data from llama-server: {_terminal_safe(data[:500])}") from exc

                    if chunk.get("error"):
                        raise _model_error(None, json.dumps({"error": chunk["error"]}))
                    response_id = chunk.get("id") or response_id
                    if isinstance(chunk.get("usage"), dict):
                        usage.update(chunk["usage"])
                    if isinstance(chunk.get("timings"), dict):
                        timings.update(chunk["timings"])
                    choices = chunk.get("choices") or []
                    if not choices:
                        continue
                    finish_reason = choices[0].get("finish_reason") or finish_reason
                    delta = choices[0].get("delta") or {}

                    thought = delta.get("reasoning_content", delta.get("reasoning"))
                    if isinstance(thought, str) and thought:
                        reasoning.append(thought)
                        if on_thought is not None:
                            clear_status()
                            on_thought(thought)

                    text = delta.get("content")
                    if isinstance(text, str) and text:
                        clear_status()
                        content.append(text)
                        on_text(text)

                    for fragment in delta.get("tool_calls") or []:
                        index = int(fragment.get("index", 0))
                        entry = tool_fragments.setdefault(
                            index,
                            {"id": "", "type": "function", "function": {"name": "", "arguments": ""}},
                        )
                        if fragment.get("id"):
                            entry["id"] = fragment["id"]
                        function = fragment.get("function") or {}
                        if function.get("name"):
                            entry["function"]["name"] += str(function["name"])
                        arguments = function.get("arguments")
                        if isinstance(arguments, str):
                            entry["function"]["arguments"] += arguments
                        elif arguments is not None:
                            entry["function"]["arguments"] += json.dumps(arguments, ensure_ascii=False)
        except urllib.error.HTTPError as exc:
            detail = exc.read(16_000).decode("utf-8", errors="replace")
            raise _model_error(exc.code, detail) from exc
        except (OSError, urllib.error.URLError) as exc:
            raise ModelError(f"Could not get a response from the local model: {exc}") from exc
        finally:
            clear_status()

        result: dict[str, Any] = {"role": "assistant", "content": "".join(content)}
        if finish_reason:
            result["_finish_reason"] = finish_reason
        if reasoning:
            result["reasoning_content"] = "".join(reasoning)
        if tool_fragments:
            calls = []
            for index in sorted(tool_fragments):
                call = tool_fragments[index]
                if not call["id"]:
                    call["id"] = f"call_{response_id or 'local'}_{index}"
                calls.append(call)
            result["tool_calls"] = calls
        if usage or timings:
            result["_metrics"] = {"usage": usage, "timings": timings}
        return result

    def _read_server_log(self) -> str:
        if self._log_file is None or not self._log_file.exists():
            return ""
        try:
            lines = self._log_file.read_text(encoding="utf-8", errors="replace").splitlines()
            return "\n".join(_terminal_safe(line) for line in lines[-50:])[-12_000:]
        except OSError:
            return ""

    def close(self) -> None:
        process = self.process
        self.process = None
        try:
            if process is not None and process.poll() is None:
                try:
                    process.terminate()
                except OSError:
                    pass
                try:
                    process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    try:
                        process.kill()
                    except OSError:
                        pass
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        pass
        finally:
            job = self._job
            self._job = None
            if job is not None:
                job.close()
            temp_dir = self._temp_dir
            self._temp_dir = None
            if temp_dir is not None:
                temp_dir.cleanup()

    def __exit__(self, exc_type: Any, exc: Any, traceback: Any) -> None:
        self.close()


def _model_error(status: int | None, detail: str) -> ModelError:
    """Turn an llama-server error body into a ModelError, recognizing context overflows."""
    message = detail.strip()
    error: Any = None
    try:
        payload = json.loads(detail)
        if isinstance(payload, dict):
            error = payload.get("error", payload)
    except json.JSONDecodeError:
        pass
    if isinstance(error, dict):
        message = str(error.get("message") or message)
        if error.get("type") == "exceed_context_size_error":
            return ContextOverflowError(
                _terminal_safe(message),
                prompt_tokens=_as_int(error.get("n_prompt_tokens")),
                context_tokens=_as_int(error.get("n_ctx")),
            )
    elif isinstance(error, str):
        message = error
    prefix = f"Local model returned HTTP {status}" if status else "Local model error"
    return ModelError(f"{prefix}: {_terminal_safe(message)[:2000]}")


def _as_int(value: Any) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return 0


# C0 controls (except newline and tab), DEL, and C1 controls can start terminal escape sequences.
_CONTROL_ESCAPES = {
    code: f"\\x{code:02x}"
    for code in (*range(32), *range(127, 160))
    if chr(code) not in "\n\t"
}


def _terminal_safe(value: str) -> str:
    """Make model/tool output unable to inject terminal control sequences."""
    return value.translate(_CONTROL_ESCAPES)
