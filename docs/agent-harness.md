# Agent harness design

## Why the Python controller owns tools

The local server is only an inference backend. It listens on loopback on an ephemeral port, uses a per-run API key, runs offline, and has its Web UI and built-in agent tools disabled. The Python controller sends a narrow function list, validates the model's response, enforces workspace paths and size limits, and mediates tool calls.

Read-only tools inspect files in the selected workspace. The session starts in `/permission ask`, where file creation, replacement, and deletion show a proposed diff and require approval; PowerShell commands also require approval and display their exact command and working directory. `/permission workspace` auto-applies workspace file changes after showing their diff but still asks before shell commands. `/permission full` skips per-action approval for the current session.

The agent starts with web tools disabled. `/web auto`, `-EnableWeb`, or `--web-mode auto` enables research for the current run. The terminal shows each query sent to DuckDuckGo and each public HTTPS page request. Switching to a different workspace restores the `ask` file and shell permissions.

On Windows, the model server and assistant-launched commands are attached to a Job Object configured with `KILL_ON_JOB_CLOSE`. Closing the CLI window stops everything automatically.

## Model detection

Runtime flags and request fields come from GGUF metadata, not file names (see [model-setup.md](model-setup.md#how-models-are-detected)). A model whose chat template supports thinking gets `--reasoning on`, its recommended sampling, and reasoning-effort fields; other models get `--reasoning off` and low-temperature sampling. A built-in MTP head turns on speculative decoding automatically.

## Reasoning controls

| `/effort` | Sent to llama.cpp |
| :-- | :-- |
| `none` | `chat_template_kwargs: {"enable_thinking": false}` |
| `low` / `medium` / `high` | `reasoning_effort` plus `thinking_budget_tokens` (omitted when the budget is 0, meaning no limit) |

`none` must use the template switch: llama.cpp treats `reasoning_effort: "none"` as unset, and the Qwen3.8 template then falls back to its deepest effort (`xhigh`). Qwen3.8 maps `high` to `xhigh`; `medium` adds no instruction.

The reasoning-effort instruction sits at the very start of the Qwen3.8 prompt, so changing `/effort` mid-session re-processes the whole prompt once.

## Context management

- **Exact accounting**: every response reports `usage.prompt_tokens` and `completion_tokens`. The harness uses those as the size of the conversation so far and estimates only messages added since (about 3 characters per token). The earlier character heuristic ignored the tool schemas and past reasoning, so it compacted too late (context-overflow errors) or too early (lost history).
- **When to compact**: before a request whose prompt would leave less than a quarter of the window (at most 4,096 tokens) for the reply. Compaction targets half the window so it does not repeat on the next round.
- **How**: turns before the current request become a short summary (requests, files actually written, commands actually run, last answer) stored as a user/assistant pair. Qwen3.8's template raises `System message must be at the beginning.` for later system messages, so the summary cannot be one. The current turn is kept from its user message onward so a tool result is never separated from its call. If it is still too large, earlier reasoning in the turn is dropped, then older tool outputs are truncated, newest last.
- **Overflow**: llama.cpp answers an oversized request with HTTP 400 `exceed_context_size_error` and the exact prompt size. The harness calibrates on that number, compacts, and retries once.
- **Reasoning in history**: Qwen3.8's template preserves reasoning across turns, so `reasoning_content` stays in history (it also keeps the prompt prefix stable for the KV cache) and counts toward the budget.
- **Tool-output limits** scale with the window: about 30% of the context per result (≈14,700 characters at 16K). The old fixed 6,000-character budget per turn often left no room to read a second file.

## Tool calls

- **Truncated calls**: when a response hits the output limit inside a tool call, its JSON arguments are incomplete. Kept verbatim in history, llama-server rejects every later request with `Failed to parse tool call arguments as JSON` until the chat is reset. The harness stores such calls with empty arguments and tells the model that nothing ran and to write smaller pieces. Saved sessions are cleaned the same way when loaded.
- **Interrupted turns**. Ctrl+C during a tool call records a result for each pending call. An interrupted model stream cannot dispatch a partially received function call. The agent keeps unanswered user input and completed tool results when a turn is interrupted or the server returns an error.
- **Summary after the tool-call limit** keeps the same tool list with `tool_choice: "none"`. Sending no tools would change the prompt prefix and force the whole conversation to be re-processed.
- **Edits over rewrites**: the system prompt asks for `replace_text` when changing existing files and manageable `write_file` calls. On the 27B, output runs at about 3 tokens per second, so rewriting a 200-line file to change one line costs roughly ten minutes.
- **Code blocks without tool calls**: weaker models sometimes print a file in markdown instead of calling `write_file`. The harness turns such a block into a `write_file` call only when the file does not exist yet, the target is unambiguous, and the fence language matches the extension. An explanation that quotes part of an existing file can never overwrite it, and a shell snippet next to `python app.py` is not saved as `app.py`.
- **Legacy formats**: the bundled `llama-server` uses the GGUF's Jinja template with `--jinja`. The harness also accepts Qwen's textual `<tool_call>` XML, JSON in markdown fences, and `tool_name({...})` text as a fallback.

## Streaming display

Answer text, reasoning, and ordinary code blocks stream as they arrive. Only a fence that could contain a JSON or XML tool call is held until it closes, then shown unless it parses as a tool call. `<tool_call>` markup is never shown.

## Sessions

Each conversation has its own ID in `%LOCALAPPDATA%\LocalAssistant\sessions\history.sqlite3`. SQLite is included with Python and runs in the CLI process. The agent saves the accepted request before generation, completed tool rounds before the next inference call, and the final reply at the end of the turn. Each save commits the transcript, its revision and the workspace’s active chat together. The trade-off is a shared database file for backup and inspection. [SQLite atomic commit](https://www.sqlite.org/atomiccommit.html)

- **Resume.** Startup reopens the active chat. `/resume` and `/resume list` read metadata in save-time order. The numbers refer to the last list displayed in this window, even if another window saves a chat. `/resume 2` switches to the original workspace and loads the selected conversation. If that folder has moved, open its new location and use `/resume 2 --here` to continue a separate copy there.
- **Fresh chats.** `/new` records an empty active selection. Earlier chats remain available and an immediate restart still opens an empty chat.
- **Concurrent windows.** Saves check the loaded revision. If two windows edit the same revision, the later save creates a separate chat so both histories survive. Clean exits skip transcript writes. A save error keeps the chat in memory and is reported in the terminal.
- **Upgrade.** At startup the agent imports the older profile JSON files, including timestamped archives. Opening a workspace also imports its legacy `.local_assistant\chat-session.json`. Each original is removed after its transaction commits. A content-based ID makes interrupted imports repeatable. Malformed JSON stays on disk for recovery.
- **Deletion.** `/delete` covers the current workspace and `/delete all` covers the profile store. Both ask for confirmation. The database enables SQLite’s `secure_delete` option. Backup copies and storage snapshots have their own retention policies. [SQLite secure deletion](https://www.sqlite.org/pragma.html#pragma_secure_delete)

## Local performance

A shorter fixed instruction block leaves more room for project guidance and conversation. The tool schema size estimate is computed once for each web mode. Keeping the shared prompt prefix stable between rounds allows llama.cpp to reuse cached tokens. [llama.cpp prompt caching](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)

Workspace listing and text search use `os.scandir()` and carry each entry’s size from the directory scan into the file tools. On Windows, ordinary entries expose these attributes in the enumeration result. Streamed function arguments are collected in fragments and joined once when the server finishes. Earlier hardware measurements and runtime settings are in the model notes. [Python directory scanning](https://docs.python.org/3/library/os.html#os.scandir) · [Model setup and benchmarks](model-setup.md#tuning-for-this-laptop)

## Primary references

- [llama.cpp server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
- [llama.cpp speculative decoding](https://github.com/ggml-org/llama.cpp/blob/master/docs/speculative.md)
- [Qwen3 official repository](https://github.com/QwenLM/Qwen3)
- [Qwen 2.5 Coder](https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct-GGUF)
- [Unsloth Qwen3.8 GGUF](https://huggingface.co/unsloth/Qwen3.8-27B-GGUF)
