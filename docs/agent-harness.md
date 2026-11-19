# Agent harness

## Terminal and runtime

Luma is a native Rust executable. Its editor draws beneath the conversation in the terminal’s normal scrollback. Crossterm supplies key, paste, resize, and cursor events. The same slash commands can be typed on line-oriented stdin when a terminal is unavailable. The interaction design uses a filterable slash menu, Shift+Tab mode switching, and a visible permission indicator. [Claude Code interactive controls](https://code.claude.com/docs/en/interactive-mode) · [Crossterm events](https://docs.rs/crossterm/latest/crossterm/event/)

The release build links SQLite and Rust dependencies into the application. This adds a Rust/C build toolchain requirement for developers. Running a built installation requires the executable, a compatible llama.cpp distribution, and local model weights. PowerShell scripts translate the original setup entry points. Cargo’s lockfile pins the build dependencies. [Cargo build](https://doc.rust-lang.org/cargo/commands/cargo-build.html)

| Module | Responsibility |
| --- | --- |
| `src/ui.rs` | Draft editing, slash completion, filtered menus, mode shortcuts, terminal restoration |
| `src/agent.rs` | Commands, approval modes, inference rounds, context accounting |
| `src/history.rs` | Transcript repair, compaction, legacy tool formats, streamed text display |
| `src/model.rs` | GGUF metadata, model discovery, sampling and runtime settings |
| `src/runtime.rs`, `src/process.rs` | Private llama-server, streaming HTTP, cancellation and process cleanup |
| `src/workspace.rs`, `src/web.rs` | Scoped file tools, diff review, PowerShell, opt-in public research |
| `src/sessions.rs` | SQLite saves, conflict handling, resume and legacy import |
| `src/setup.rs` | Explicit runtime and weight downloads, command installation and plain chat |

## Inference and cancellation

The model server starts when a prompt first needs inference. The controller binds it to `127.0.0.1` on an ephemeral port, generates an API key, and starts it offline with the server’s web UI and built-in agent disabled. The inference HTTP client bypasses proxies and rejects redirects. Model weights and the server remain independent of the controller executable. [llama.cpp server API](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)

Stream chunks carry answer text, reasoning, function fragments, and usage. Luma consumes them through a bounded channel while the terminal thread watches for cancellation. A missing finish reason discards partial tool calls. Interrupting a generation stops the server process. The next request reloads it. On Windows the server and each approved shell launch have their own kill-on-close Job Object, including child processes.

The controller keeps the server’s prompt and completion token count for the known message prefix and estimates new messages until the next response. When space runs low it summarizes older turns, retaining the current user request and matching tool results. It can remove older reasoning and shorten tool output. A context overflow triggers one compaction retry. Answer-length continuations are limited to two. The agent permits up to eight tool rounds and twelve calls per user request, then asks for a summary.

## Tool execution

The controller sends the schemas allowed in the current mode and checks the tool name again before dispatch. Plan mode exposes `list_files`, `read_file` and `search_text`. Each tool argument is validated by its handler. In Ask mode writes and deletions require an affirmative answer after displaying the diff. PowerShell displays the exact command and working directory. The README covers the other permission modes.

The file policy checks each path component, rejects junctions and symlinks, and applies common sensitive-file exclusions and the root `.gitignore`. Writes check the reviewed original a second time and publish a temporary file in the destination directory. New-file writes use a no-clobber operation. Text tools limit reads to 2 MiB.

Native structured function calls are preferred. For local models that emit tools as text, the controller recognizes JSON, tagged Qwen calls, JSON fences, and function-call notation. A code fence can become a new-file proposal when the request explicitly names one compatible target. It goes through the same permission checks. Incomplete arguments are stored in a template-safe form and receive an explanatory tool result.

Web tools are off on startup. When explicitly enabled, DuckDuckGo gets the search query, and the destination site gets the page request. The web client accepts public HTTPS, validates resolved addresses, pins the connection to those addresses, and validates each redirect separately. Search results, pages, files and command output are untrusted inputs to the model.

## Session compatibility

Luma opens the existing `%LOCALAPPDATA%\LocalAssistant\sessions\history.sqlite3` schema and uses the same Windows workspace keys as the previous controller. No database export is required. Each save updates the transcript, revision and workspace’s active chat in one transaction. The portable build uses bundled SQLite through rusqlite. [rusqlite](https://docs.rs/rusqlite/latest/rusqlite/) · [SQLite atomic commit](https://www.sqlite.org/atomiccommit.html)

- The controller saves an accepted user request before inference, completed tool rounds before the next generation, and the final answer. Concurrent edits to one revision create separate chats. A failed save retains the in-memory conversation.
- `/resume` provides a filterable selection. `/resume list` keeps a numbered snapshot for the current window. `/resume N --here` copies a chat into a new workspace. Permissions reset to Ask when switching to a different project.
- `/new` records an empty workspace selection. Quitting immediately leaves that selection intact on the next launch.
- Existing profile JSON and timestamped archives import at startup. Opening a workspace imports its legacy `.local_assistant\chat-session.json`. A content-based ID makes imports repeatable after an interruption. The source is removed after commit if its size and modification time still match.
- `/delete` and `/delete all` require confirmation. SQLite secure deletion is enabled. Backups and filesystem snapshots have independent retention. [SQLite secure deletion](https://www.sqlite.org/pragma.html#pragma_secure_delete)
