# Agent harness design

## Research and backend choice

The bundled `llama-server` build exposes an OpenAI-compatible chat-completions API. Its documentation says function calling uses the Jinja template path (`--jinja`) and may need a tool-compatible template. The Qwen3.8 deployment examples show `qwen3_coder` parsing for vLLM and SGLang; that parser handles XML-style calls such as `<tool_call><function=...><parameter=...>...`. This harness uses the GGUF's embedded Jinja template with llama.cpp and converts either structured calls or that textual Qwen format into its own validated tool calls. Exact behavior depends on the local GGUF template and llama.cpp build, so tool selection should be evaluated on actual tasks.

## Why the Python controller owns tools

The local server is only an inference backend. It listens on loopback on an ephemeral port, uses a per-run API key, runs offline, and has its Web UI and built-in agent tools disabled. The Python controller sends a narrow function list, validates the model's response, enforces workspace paths and size limits, and mediates tool calls.

Read-only tools inspect files in the selected workspace. The session starts in `/permission ask`, where file creation, replacement, and deletion show a proposed diff and require the user to type `YES`; PowerShell commands also require approval and display their exact command and working directory. `/permission workspace` auto-applies workspace file changes after showing their diff but still asks before shell commands. `/permission full` skips per-action approval for file changes and commands for the current session. Commands run with the signed-in Windows user's permissions and can affect files outside the workspace; the harness is not an operating-system sandbox. Use `/permission ask` to restore prompts. Web research defaults to automatic tool availability: the model is instructed to search for explicit lookups, current facts, and questions it cannot answer reliably, and to avoid sending workspace contents or private context. Search queries go to DuckDuckGo, each web tool call is displayed in the CLI, and only public HTTPS pages are fetched. Use `/web off` or `--web-mode off` to disable it; `/web on` restores automatic lookups for the session.

On Windows, `scripts/install-command.ps1` installs the `luma` and `local-assistant` terminal commands in the user's local Programs directory and adds that directory to the user's PATH. The launcher uses the current directory as the workspace unless a workspace path is supplied.

On Windows, the local model server and assistant-run shell commands are attached to a Job Object configured with `KILL_ON_JOB_CLOSE`. Normal CLI exit stops them through the context manager; closing or terminating the launcher also closes the Job Object, which makes Windows stop those processes and their child processes instead of leaving work running in the background.

Tool output and fetched pages are untrusted input. They cannot approve actions or change the tool policy. Common secrets and binary model files are excluded from file tools. Conversation context remains in memory and is discarded when the CLI exits.

## Local performance tuning

The Python CLI now reports prompt prefill and generated-token throughput from llama.cpp's streaming usage/timing data. It exposes the model path, context, CPU threads, VRAM fit reserve, KV-cache type, reasoning effort/budget, and optional speculative decoder so the settings can be compared on this machine. Use `/effort none|low|medium|high` for reasoning depth and `/effort budget N` for the token budget.

I ran same-prompt single-turn comparisons on the Ryzen 5 7640HS / RTX 4050 Laptop GPU (6 GB) with the 8K context and a 64-token thinking budget. Results vary by run and answer length, so treat these as local observations, not a formal benchmark:

| Configuration | Prefill | Generation |
| --- | ---: | ---: |
| Default fit reserve 1024 MiB, F16 KV, automatic threads, no speculation | 64.1 tok/s | 2.1 tok/s |
| 512 MiB reserve, Q8 KV, 6 CPU threads, ngram-mod | 56.9 tok/s | 2.1 tok/s |
| Same tuned settings with Qwen MTP draft | 58.0 tok/s | 1.8 tok/s |

The MTP run accepted 58 of 96 draft tokens (60%). This did not turn into a measured throughput gain for this workload, so the default stays F16 KV, automatic CPU threads, 1024 MiB reserve, and no speculative decoding. Try the draft manually with `scripts/start-agent.ps1 -Speculative draft-mtp`; the 1.37 GB file is downloaded and SHA-256 verified by `scripts/download-model.ps1 -MTP`.

Generation remained near 2 tok/s across these configurations, including a direct-response run with reasoning disabled. The 27B Q4 model is substantially larger than the laptop's VRAM, so most weights still rely on system memory. The harness settings cannot remove that hardware bottleneck; a smaller quant or model would be a separate quality/performance choice. Use `/effort none` or a smaller `/effort budget N` for routine responses, and raise them for harder tasks. These were one-off short-prompt runs, not a statistical benchmark or an agent-tool correctness evaluation.

## Primary references

- [llama.cpp server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md) — local API, Jinja tool calls, API keys, loopback binding, and built-in tool flags.
- [llama.cpp speculative decoding documentation](https://github.com/ggml-org/llama.cpp/blob/master/docs/speculative.md) — MTP and draftless decoder options.
- [llama.cpp reasoning and streaming timing options](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md) — per-request reasoning controls, stream usage and decode timing fields.
- [llama.cpp per-request thinking budget discussion](https://github.com/ggml-org/llama.cpp/discussions/21445) — `thinking_budget_tokens` support when no server-wide budget is set.
- [Qwen3.8 official repository](https://github.com/QwenLM/Qwen3.8) — model deployment examples and `qwen3_coder` parser configuration for SGLang and vLLM.
- [vLLM Qwen3 parser implementation](https://github.com/vllm-project/vllm/blob/main/vllm/parser/qwen3.py) — the XML-style function and parameter tags parsed by the compatibility fallback.
- [Unsloth Qwen3.8 GGUF repository](https://huggingface.co/unsloth/Qwen3.8-27B-GGUF) — local GGUF and llama.cpp usage.
- [Unsloth Qwen3.8 MTP draft](https://huggingface.co/unsloth/Qwen3.8-27B-GGUF/blob/main/MTP/mtp-Qwen3.8-27B-Q4_0.gguf) — optional 1.37 GB Q4 draft head.
