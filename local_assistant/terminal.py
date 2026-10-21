"""Small, dependency-free terminal welcome and slash-command input UI."""

from __future__ import annotations

import math
import os
import shutil
import sys
from pathlib import Path
from typing import Any


_VT_READY = False

_LOGO = (
    "██╗      ██████╗  ██████╗ █████╗ ██╗      █████╗ ██╗",
    "██║     ██╔═══██╗██╔════╝██╔══██╗██║     ██╔══██╗██║",
    "██║     ██║   ██║██║     ███████║██║     ███████║██║",
    "██║     ██║   ██║██║     ██╔══██║██║     ██╔══██║██║",
    "███████╗╚██████╔╝╚██████╗██║  ██║███████╗██║  ██║██║",
    "╚══════╝ ╚═════╝  ╚═════╝╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚═╝",
)

SLASH_HELP = (
    ("/help", "Show this command list"),
    ("/new", "Start a fresh conversation (/clear also works)"),
    ("/workspace", "Show the folder the assistant can access"),
    ("/permission", "Choose ask, workspace, or full access for this session"),
    ("/effort", "Set reasoning depth or its token budget"),
    ("/web auto", "Allow automatic web lookups when useful"),
    ("/web off", "Disable web access for this session"),
    ("/exit", "Stop the assistant (/quit also works)"),
)

SLASH_COMPLETIONS = (
    ("/help", "Show available commands"),
    ("/new", "Start a fresh conversation"),
    ("/workspace", "Show the active workspace folder"),
    ("/permission", "Show the current permission mode"),
    ("/permission ask", "Ask before every file change and shell command"),
    ("/permission workspace", "Auto-apply workspace edits; ask before shell commands"),
    ("/permission full", "Run file changes and commands without per-action approval"),
    ("/effort", "Show current reasoning settings"),
    ("/effort none", "Fast, direct answers"),
    ("/effort low", "Light reasoning"),
    ("/effort medium", "Balanced reasoning"),
    ("/effort high", "Deeper reasoning"),
    ("/effort budget ", "Set a reasoning-token budget, for example 512"),
    ("/web auto", "Use web lookups when a question needs them"),
    ("/web off", "Disable web access for this session"),
    ("/web on", "Alias for /web auto"),
    ("/exit", "Stop the assistant"),
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


def display_model_name(model_file: Path) -> str:
    stem = model_file.stem
    if "-UD-" in stem:
        return stem.split("-UD-")[0]
    for tag in ("-q4", "-Q4", "-q8", "-Q8", "-f16", "-F16"):
        if tag in stem:
            return stem.split(tag)[0]
    return stem


def print_welcome(workspace: Path, model_file: Path, context_length: int, web_mode: bool) -> None:
    color = sys.stdout.isatty() and _VT_READY
    cyan = "\x1b[96m" if color else ""
    white = "\x1b[1;97m" if color else ""
    muted = "\x1b[90m" if color else ""
    reset = "\x1b[0m" if color else ""

    model_name = display_model_name(model_file)
    context = f"{context_length // 1024}K context"
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
    if web_mode:
        print(f"  Web         Auto lookups  ·  shown before each search")
    else:
        print("  Web         Off  ·  this session is offline")
    print()


def print_command_menu() -> None:
    width = max(48, shutil.get_terminal_size((100, 30)).columns)
    print("Available commands")
    print("─" * min(width, 76))
    for command, description in SLASH_HELP:
        print(f"  {command:<27} {description}")
    print("\nType / to browse and filter commands; use ↑/↓, Tab, and Enter.\n")


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

    def matches() -> list[tuple[str, str]]:
        if dismissed or not buffer.startswith("/"):
            return []
        prefix = buffer.casefold()
        return [entry for entry in SLASH_COMPLETIONS if entry[0].casefold().startswith(prefix)]

    def redraw() -> None:
        nonlocal previous_cursor_row
        line = prompt + buffer
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
            menu_header = "  COMMANDS  ·  ↑↓ select  ·  Tab complete  ·  Enter run  ·  Esc hide"
            sys.stdout.write("\r\n")
            sys.stdout.write("\x1b[2m" + menu_header[:width - 1] + "\x1b[0m\r\n")
            for index, (command, description) in enumerate(visible):
                marker = "›" if index == selected else " "
                row = f" {marker} {command:<25} {description}"
                row = row[:width - 1]
                if index == selected:
                    sys.stdout.write("\x1b[1;97m" + row + "\x1b[0m\r\n")
                else:
                    sys.stdout.write(row + "\r\n")
            menu_rows = 1 + len(visible)
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
        if key == "\r":
            options = matches()
            if options and buffer != options[selected % len(options)][0]:
                buffer = options[selected % len(options)][0]
                cursor = len(buffer)
            # Remove the suggestion panel, leaving a normal command line in scrollback.
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
            redraw()
