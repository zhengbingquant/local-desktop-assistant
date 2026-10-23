# Agent harness design

## Why the Python controller owns tools

The local server is only an inference backend. It listens on loopback on an ephemeral port, uses a per-run API key, runs offline, and has its Web UI and built-in agent tools disabled. The Python controller sends a narrow function list, validates the model's response, enforces workspace paths and size limits, and mediates tool calls.

Read-only tools inspect files in the selected workspace. The session starts in `/permission ask`, where file creation, replacement, and deletion show a proposed diff and require approval; PowerShell commands also require approval and display their exact command and working directory. `/permission workspace` auto-applies workspace file changes after showing their diff but still asks before shell commands. `/permission full` skips per-action approval for the current session.

Web research defaults to automatic: the model searches for explicit lookups, current facts, and uncertain answers. Search queries go to DuckDuckGo, each call is displayed, and only public HTTPS pages are fetched. Use `/web off` to disable.

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
- **Interrupted turns**: Ctrl+C during a tool call still records a result for each pending call, so the history stays well-formed.
- **Summary after the tool-call limit** keeps the same tool list with `tool_choice: "none"`. Sending no tools would change the prompt prefix and force the whole conversation to be re-processed.
- **Edits over rewrites**: the system prompt asks for `replace_text` when changing existing files and for files under about 200 lines per `write_file`. On the 27B, output runs at about 3 tokens per second, so rewriting a 200-line file to change one line costs roughly ten minutes.
- **Code blocks without tool calls**: weaker models sometimes print a file in markdown instead of calling `write_file`. The harness turns such a block into a `write_file` call only when the file does not exist yet, the target is unambiguous, and the fence language matches the extension. An explanation that quotes part of an existing file can never overwrite it, and a shell snippet next to `python app.py` is not saved as `app.py`.
- **Legacy formats**: the bundled `llama-server` uses the GGUF's Jinja template with `--jinja`. The harness also accepts Qwen's textual `<tool_call>` XML, JSON in markdown fences, and `tool_name({...})` text as a fallback.

## Streaming display

Answer text, reasoning, and ordinary code blocks stream as they arrive. Only a fence that could contain a JSON or XML tool call is held until it closes, then shown unless it parses as a tool call. `<tool_call>` markup is never shown.

## Sessions

Conversations are saved after every turn to `%LOCALAPPDATA%\LocalAssistant\sessions\<hash of the workspace path>.json` and resumed automatically when you start in the same folder. `/new` deletes the saved conversation. Earlier versions wrote `.local_assistant\chat-session.json` inside the workspace, where a transcript could be committed by accident; that file is moved to the new location the first time it is loaded.

## Local performance

Benchmarks and the reasoning behind each runtime setting are in [model-setup.md](model-setup.md#tuning-for-this-laptop).

## Primary references

- [llama.cpp server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
- [llama.cpp speculative decoding](https://github.com/ggml-org/llama.cpp/blob/master/docs/speculative.md)
- [Qwen3.8 official repository](https://github.com/QwenLM/Qwen3.8)
- [Qwen 2.5 Coder](https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct-GGUF)
- [Unsloth Qwen3.8 GGUF](https://huggingface.co/unsloth/Qwen3.8-27B-GGUF)
