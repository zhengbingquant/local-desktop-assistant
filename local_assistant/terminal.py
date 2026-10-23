"""Small, dependency-free terminal welcome, slash-command input, and interactive menus."""

from __future__ import annotations

import math
import os
import shutil
import sys
from pathlib import Path
from typing import Any

from .models import ModelInfo, RuntimeTuning, describe_runtime

if hasattr(sys.stdout, "reconfigure"):
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass
if hasattr(sys.stderr, "reconfigure"):
    try:
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass

_VT_READY = False

_LOGO = (
    "██╗      ██████╗  ██████╗ █████╗ ██╗      █████╗ ██╗",
    "██║     ██╔═══██╗██╔════╝██╔══██╗██║     ██╔══██╗██║",
    "██║     ██║   ██║██║     ███████║██║     ███████║██║",
    "██║     ██║   ██║██║     ██╔══██║██║     ██╔══██║██║",
    "███████╗╚██████╔╝╚██████╗██║  ██║███████╗██║  ██║██║",
    "╚══════╝ ╚═════╝  ╚═════╝╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚═╝",
)


# ── Slash command definitions ────────────────────────────────────────
# Clean root commands (what appears when typing '/')
_COMMAND_GROUPS = (
    ("Model & Reasoning", (
        ("/model",              "Switch active model or view model details", "🤖"),
        ("/think",              "Toggle thinking process display (show / hide)", "💭"),
        ("/effort",             "Set reasoning effort or thinking budget",         "🧠"),
    )),
    ("Environment", (
        ("/workspace",          "View or switch active workspace folder",          "📁"),
        ("/permission",         "Switch permission mode (ask / workspace / full)", "🔒"),
        ("/web",                "Configure web research (auto / off)",            "🌐"),
    )),
    ("Session & Context", (
        ("/context",            "Show context window usage and details",           "📊"),
        ("/compact",            "Compact conversation history to save tokens",     "🗜️"),
        ("/save",               "Save the current conversation to disk",           "💾"),
        ("/resume",             "Resume the last saved conversation",              "↩️"),
        ("/new",                "Start a fresh conversation (/clear)",             "⟳"),
        ("/cls",                "Clear the terminal screen",                       "🧹"),
        ("/help",               "Show available commands",                         "?"),
        ("/exit",               "Stop the assistant (/quit)",                      "⏻"),
    )),
)

# Extended completions for direct typing (e.g. /permission ask, /model 7b)
_EXTENDED_COMPLETIONS = (
    ("/model 27b", "Switch to Qwen3.8 27B (default · reasoning · GPU + system RAM)"),
    ("/model 7b", "Switch to Qwen 2.5 Coder 7B (fast · fits entirely in VRAM)"),
    ("/think show", "Stream thinking and reasoning live in dimmed text"),
    ("/think hide", "Hide thinking process (show 'Thinking...' status only)"),
    ("/permission ask", "Ask before every file change and command"),
    ("/permission workspace", "Auto-apply workspace file edits; ask for commands"),
    ("/permission full", "Run file changes and commands without per-action approval"),
    ("/effort none", "Fast, direct answers — no thinking"),
    ("/effort low", "Light reasoning"),
    ("/effort medium", "Balanced reasoning (default)"),
    ("/effort high", "Deeper reasoning — slower but thorough"),
    ("/effort budget ", "Set token budget, e.g. /effort budget 512"),
    ("/web auto", "Enable automatic web lookups"),
    ("/web off", "Disable web — fully offline session"),
    ("/save", "Save the current conversation to disk for later resume"),
    ("/resume", "Resume the last saved conversation for this workspace"),
    ("/workspace ", "Switch active workspace folder, e.g. /workspace C:\\project"),
)

# Flattened for autocomplete matching
SLASH_COMPLETIONS: tuple[tuple[str, str], ...] = tuple(
    (cmd, desc)
    for _, entries in _COMMAND_GROUPS
    for cmd, desc, _ in entries
) + _EXTENDED_COMPLETIONS

# Flattened with icons for the interactive '/' popup
_SLASH_MENU_ENTRIES: tuple[tuple[str, str, str, str], ...] = tuple(
    (cmd, desc, group, icon)
    for group, entries in _COMMAND_GROUPS
    for cmd, desc, icon in entries
)

# Static help output (for /help and non-interactive terminals)
SLASH_HELP = tuple(
    (cmd, desc)
    for _, entries in _COMMAND_GROUPS
    for cmd, desc, _ in entries
)


def enable_terminal_features() -> tuple[Any, Any, int, bool] | None:
    """Enable ANSI sequences on Windows terminals; return state for restoration."""
    global _VT_READY
    if not sys.stdout.isatty():
        return None
    if os.name != "nt":
        _VT_READY = True
        return None
    try:
        import ctypes
        from ctypes import wintypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.GetStdHandle.argtypes = [wintypes.DWORD]
        kernel32.GetStdHandle.restype = wintypes.HANDLE
        kernel32.GetConsoleMode.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
        kernel32.GetConsoleMode.restype = wintypes.BOOL
        kernel32.SetConsoleMode.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel32.SetConsoleMode.restype = wintypes.BOOL

        handle = kernel32.GetStdHandle(wintypes.DWORD(0xFFFFFFF5))  # STD_OUTPUT_HANDLE (-11)
        mode = wintypes.DWORD()
        if not handle or not kernel32.GetConsoleMode(handle, ctypes.byref(mode)):
            return None
        original_mode = int(mode.value)
        virtual_terminal = 0x0004
        changed = not (original_mode & virtual_terminal)
        if changed and not kernel32.SetConsoleMode(handle, original_mode | virtual_terminal):
            return None
        _VT_READY = True
        return (kernel32, handle, original_mode, changed)
    except (AttributeError, OSError, ImportError):
        return None


def restore_terminal_features(state: tuple[Any, Any, int, bool] | None) -> None:
    global _VT_READY
    if state:
        kernel32, handle, original_mode, changed = state
        if changed:
            kernel32.SetConsoleMode(handle, original_mode)
    _VT_READY = False


def is_color_enabled() -> bool:
    return sys.stdout.isatty() and (_VT_READY or os.name != "nt")


def print_welcome(workspace: Path, model: ModelInfo, tuning: RuntimeTuning, web_mode: bool) -> None:
    color = sys.stdout.isatty() and _VT_READY
    cyan = "\x1b[96m" if color else ""
    white = "\x1b[1;97m" if color else ""
    muted = "\x1b[90m" if color else ""
    reset = "\x1b[0m" if color else ""

    model_name = model.name
    context = f"{tuning.context_length // 1024}K context" + ("  ·  reasoning" if model.reasoning else "")
    print()
    for line in _LOGO:
        print(f"{cyan}{line}{reset}")
    print(f"{white}LOCAL ASSISTANT{reset}  {muted}·  {model_name}{reset}")
    print(f"{muted}{'─' * 58}{reset}")
    print(f"{white}Hello! What are we working on today?{reset}")
    print(f"{muted}Your private AI workspace, running on this laptop.{reset}")
    print(f"{muted}Permission ask · file edits and shell commands request approval.{reset}")
    print()
    print(f"  Workspace   {workspace}")
    print(f"  Model       {model_name}  ·  {context}")
    print(f"  Runtime     {describe_runtime(model, tuning)}")
    if web_mode:
        print(f"  Web         Auto lookups  ·  shown before each search")
    else:
        print("  Web         Off  ·  this session is offline")
    print()


def print_command_menu() -> None:
    color = sys.stdout.isatty() and _VT_READY
    white = "\x1b[1;97m" if color else ""
    muted = "\x1b[90m" if color else ""
    cyan = "\x1b[96m" if color else ""
    reset = "\x1b[0m" if color else ""

    width = max(48, shutil.get_terminal_size((100, 30)).columns)
    separator = "─" * min(width, 76)

    print(f"\n{white}Commands{reset}")
    print(f"{muted}{separator}{reset}")

    last_group = ""
    for group, entries in _COMMAND_GROUPS:
        if group != last_group:
            if last_group:
                print()
            print(f"  {muted}{group}{reset}")
            last_group = group
        for cmd, desc, icon in entries:
            print(f"    {cyan}{cmd:<25}{reset} {muted}{icon}  {desc}{reset}")

    print(f"\n{muted}  Type / to browse commands · ↑↓ navigate · Tab complete · Enter run{reset}\n")


def select_menu(
    title: str,
    options: list[tuple[str, str, str]],  # (id, label, description)
    current: str | None = None,
) -> str | None:
    """Show an interactive in-place radio selection menu using arrow keys."""
    if os.name != "nt" or not _VT_READY or not sys.stdin.isatty() or not sys.stdout.isatty():
        print(f"\n{title}")
        print("─" * 40)
        for i, (opt_id, label, desc) in enumerate(options, 1):
            active = " (current)" if opt_id == current else ""
            print(f"  {i}. {label:<12} {desc}{active}")
        try:
            choice = input(f"\nSelect [1-{len(options)}] or press Enter to cancel: ").strip()
            if choice.isdigit() and 1 <= int(choice) <= len(options):
                return options[int(choice) - 1][0]
        except (EOFError, KeyboardInterrupt):
            pass
        return None

    try:
        import msvcrt
    except ImportError:
        return None

    selected = 0
    if current is not None:
        for idx, (opt_id, _, _) in enumerate(options):
            if opt_id == current:
                selected = idx
                break

    width = max(30, shutil.get_terminal_size((100, 30)).columns)
    separator = "─" * min(width - 4, 64)
    total_lines = len(options) + 2

    def render(is_first: bool = False) -> None:
        if not is_first:
            sys.stdout.write(f"\r\x1b[{total_lines}A")
        sys.stdout.write("\x1b[J")
        sys.stdout.write(f"  \x1b[1;97m{title}\x1b[0m  \x1b[90m·  ↑↓ choose  ·  Enter select  ·  Esc cancel\x1b[0m\r\n")
        sys.stdout.write(f"  \x1b[90m{separator}\x1b[0m\r\n")
        for idx, (opt_id, label, desc) in enumerate(options):
            is_active = (opt_id == current)
            radio = "●" if is_active else "○"
            active_badge = " \x1b[92m(active)\x1b[0m" if is_active else ""
            if idx == selected:
                line = f"  \x1b[96m› {radio} {label:<12}\x1b[0m \x1b[1;97m{desc}\x1b[0m{active_badge}"
            else:
                line = f"  \x1b[90m  {radio} {label:<12} {desc}{active_badge}\x1b[0m"
            sys.stdout.write(line + "\r\n")
        sys.stdout.flush()

    render(is_first=True)

    try:
        while True:
            key = msvcrt.getwch()
            if key == "\r":
                sys.stdout.write(f"\r\x1b[{total_lines}A\x1b[J")
                sys.stdout.flush()
                return options[selected][0]
            if key in ("\x1b", "\x03"):
                sys.stdout.write(f"\r\x1b[{total_lines}A\x1b[J")
                sys.stdout.flush()
                return None
            if key.isdigit():
                num = int(key)
                if 1 <= num <= len(options):
                    sys.stdout.write(f"\r\x1b[{total_lines}A\x1b[J")
                    sys.stdout.flush()
                    return options[num - 1][0]
            if key in ("\x00", "\xe0"):
                code = msvcrt.getwch()
                if code in ("H", "K"):
                    selected = (selected - 1) % len(options)
                    render(is_first=False)
                elif code in ("P", "M"):
                    selected = (selected + 1) % len(options)
                    render(is_first=False)
    except (EOFError, KeyboardInterrupt):
        sys.stdout.write(f"\r\x1b[{total_lines}A\x1b[J")
        sys.stdout.flush()
        return None


def read_prompt_line(prompt: str = "You › ") -> str:
    """Read a line; on Windows, show slash suggestions as soon as '/' is typed."""
    if os.name != "nt" or not _VT_READY or not sys.stdin.isatty() or not sys.stdout.isatty():
        return input(prompt)
    try:
        import msvcrt
    except ImportError:
        return input(prompt)

    buffer = ""
    cursor = 0
    selected = 0
    dismissed = False
    previous_cursor_row = 0
    width = max(20, shutil.get_terminal_size((100, 30)).columns)

    def matches() -> list[tuple[str, str, str, str]]:
        if dismissed or not buffer.startswith("/") or "\n" in buffer:
            return []
        prefix = buffer.casefold()
        if " " not in buffer:
            return [e for e in _SLASH_MENU_ENTRIES if e[0].casefold().startswith(prefix)]
        return [
            (cmd, desc, "Options", "›")
            for cmd, desc in _EXTENDED_COMPLETIONS
            if cmd.casefold().startswith(prefix)
        ]

    def redraw() -> None:
        nonlocal previous_cursor_row
        line = prompt + buffer.replace("\n", "↵")
        cursor_position = len(prompt) + cursor
        if previous_cursor_row:
            sys.stdout.write("\r\x1b[" + str(previous_cursor_row) + "A")
        else:
            sys.stdout.write("\r")
        sys.stdout.write("\x1b[J")
        sys.stdout.write(line)

        input_rows = max(1, math.ceil(len(line) / width))
        cursor_row = cursor_position // width
        cursor_col = cursor_position % width
        suggestions = matches()
        menu_rows = 0
        if suggestions:
            max_items = max(1, min(len(suggestions), shutil.get_terminal_size((100, 30)).lines - 5))
            visible = suggestions[:max_items]
            header = "  ↑↓ select · Tab complete · Enter run · Esc close"
            sys.stdout.write("\r\n")
            sys.stdout.write("\x1b[90m" + header[:width - 1] + "\x1b[0m\r\n")
            last_group = ""
            rendered = 0
            for index, (cmd, desc, group, icon) in enumerate(visible):
                if group != last_group:
                    if last_group:
                        sys.stdout.write("\x1b[90m  ─\x1b[0m\r\n")
                        rendered += 1
                    last_group = group
                marker = "›" if index == selected else " "
                if index == selected:
                    row = f" {marker} \x1b[96m{cmd:<25}\x1b[0m \x1b[1;97m{icon}  {desc}\x1b[0m"
                else:
                    row = f" {marker} \x1b[90m{cmd:<25} {icon}  {desc}\x1b[0m"
                sys.stdout.write(row + "\r\n")
                rendered += 1
            menu_rows = 1 + rendered
            sys.stdout.write("\x1b[" + str(input_rows + menu_rows) + "A")
            if cursor_row:
                sys.stdout.write("\x1b[" + str(cursor_row) + "B")
            if cursor_col:
                sys.stdout.write("\x1b[" + str(cursor_col) + "C")
            previous_cursor_row = cursor_row
        else:
            if cursor_row:
                sys.stdout.write("\r\x1b[" + str(input_rows - 1) + "A")
                sys.stdout.write("\x1b[" + str(cursor_row) + "B")
            else:
                sys.stdout.write("\r")
            if cursor_col:
                sys.stdout.write("\x1b[" + str(cursor_col) + "C")
            previous_cursor_row = cursor_row
        sys.stdout.flush()

    redraw()
    while True:
        key = msvcrt.getwch()
        # While pasted text is still queued, skip redraws so long pastes stay fast.
        if key == "\n" or (key == "\r" and msvcrt.kbhit()):
            # Ctrl+Enter, or a line break inside pasted text: keep composing one message.
            buffer = buffer[:cursor] + "\n" + buffer[cursor:]
            cursor += 1
            dismissed = True
            if not msvcrt.kbhit():
                redraw()
            continue
        if key == "\r":
            options = matches()
            if options and buffer != options[selected % len(options)][0]:
                buffer = options[selected % len(options)][0]
                cursor = len(buffer)
            dismissed = True
            redraw()
            sys.stdout.write("\r\n")
            sys.stdout.flush()
            return buffer
        if key == "\x03":
            sys.stdout.write("\r\n")
            sys.stdout.flush()
            raise KeyboardInterrupt
        if key == "\x1b":
            dismissed = True
            redraw()
            continue
        if key in ("\x00", "\xe0"):
            code = msvcrt.getwch()
            options = matches()
            if code in ("H", "P") and options:
                step = -1 if code == "H" else 1
                selected = (selected + step) % len(options)
            elif code == "K":
                cursor = max(0, cursor - 1)
                dismissed = True if cursor < len(buffer) else dismissed
            elif code == "M":
                cursor = min(len(buffer), cursor + 1)
            elif code == "G":
                cursor = 0
            elif code == "O":
                cursor = len(buffer)
            elif code == "S" and cursor < len(buffer):
                buffer = buffer[:cursor] + buffer[cursor + 1:]
            redraw()
            continue
        if key == "\t" and msvcrt.kbhit():
            # Indentation inside pasted text, not a completion request.
            buffer = buffer[:cursor] + "    " + buffer[cursor:]
            cursor += 4
            continue
        if key == "\t":
            options = matches()
            if options:
                buffer = options[selected % len(options)][0]
                cursor = len(buffer)
                dismissed = False
            redraw()
            continue
        if key == "\b":
            if cursor:
                buffer = buffer[:cursor - 1] + buffer[cursor:]
                cursor -= 1
            selected = 0
            dismissed = False
            redraw()
            continue
        if key >= " ":
            buffer = buffer[:cursor] + key + buffer[cursor:]
            cursor += len(key)
            selected = 0
            dismissed = False
            if not msvcrt.kbhit():
                redraw()
