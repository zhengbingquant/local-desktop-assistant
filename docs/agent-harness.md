# Agent harness design

## Model auto-detection

The runtime auto-detects whether the loaded model supports reasoning (e.g., Qwen3.8, DeepSeek-R1) or is a standard coder/instruct model (e.g., Qwen 2.5 Coder). This affects:

- **Server flags**: reasoning models get `--reasoning on` with effort/budget controls; coder models get `--reasoning off`.
- **Sampling**: reasoning models use `temp=1.0, top_p=0.95, top_k=20`; coder models use `temp=0.2, top_p=0.9, top_k=40` for more deterministic code generation.
- **Token budget**: `max_tokens` is dynamically set to `min(4096, context_length - 1024)` to reduce truncation failures.
- **Request body**: reasoning-specific fields (`reasoning_effort`, `thinking_budget_tokens`) are only sent for reasoning models.

Detection is based on the model filename containing known tags (`qwen3`, `deepseek-r1`, etc.).

## Modular code generation

The system prompt instructs the model to work incrementally:
- Never write an entire large application in a single tool call.
- Keep individual file writes under 150–200 lines.
- Create a working skeleton first, then add features step by step.

This prevents the token-limit truncation crash that occurs when a model tries to generate a massive file in one shot and the JSON tool-call argument is cut off mid-string.

## Truncation error handling

When `llama-server` returns HTTP 500 with a JSON parse error (e.g., `missing closing quote`), the harness now provides a clear, actionable error message instead of dumping raw server diagnostics:

> *The model's code output hit the response token limit and was cut off mid-tool call.*
> *Tip: Ask the model to build the project modularly step-by-step or write smaller files.*

## Why the Python controller owns tools

The local server is only an inference backend. It listens on loopback on an ephemeral port, uses a per-run API key, runs offline, and has its Web UI and built-in agent tools disabled. The Python controller sends a narrow function list, validates the model's response, enforces workspace paths and size limits, and mediates tool calls.

Read-only tools inspect files in the selected workspace. The session starts in `/permission ask`, where file creation, replacement, and deletion show a proposed diff and require the user to type `YES`; PowerShell commands also require approval and display their exact command and working directory. `/permission workspace` auto-applies workspace file changes after showing their diff but still asks before shell commands. `/permission full` skips per-action approval for the current session.

Web research defaults to automatic: the model searches for explicit lookups, current facts, and uncertain answers. Search queries go to DuckDuckGo, each call is displayed, and only public HTTPS pages are fetched. Use `/web off` to disable.

On Windows, the model server and assistant-launched commands are attached to a Job Object configured with `KILL_ON_JOB_CLOSE`. Closing the CLI window stops everything automatically.

## Local performance benchmarks

Measured on AMD Ryzen 5 7640HS / RTX 4050 Laptop GPU (6 GB) with 8K context:

### Qwen 2.5 Coder 7B Instruct (Q4_K_M) — full GPU

| Metric | Value |
|:-------|------:|
| Prefill | ~120 tok/s |
| Generation | ~45 tok/s |
| Typical agent turn | 10–20 seconds |

### Qwen3.8-27B (Q4_K_XL, F16 KV) — CPU offload

| Configuration | Prefill | Generation |
|:------------- | -------:| ----------:|
| Default (1024 MiB reserve, F16 KV, no speculation) | 64.1 tok/s | 2.1 tok/s |
| 512 MiB reserve, Q8 KV, 6 CPU threads, ngram-mod | 56.9 tok/s | 2.1 tok/s |
| Same with Qwen MTP draft | 58.0 tok/s | 1.8 tok/s |

Generation for the 27B model stayed near 2 tok/s across all configurations. The bottleneck is system memory bandwidth, not a harness issue.

## Qwen tool-call compatibility

The bundled `llama-server` uses the GGUF's embedded Jinja template with `--jinja`. The harness also parses Qwen's textual XML-style `<tool_call>` format as a fallback, converting either structured calls or XML-form output into validated tool calls.

## Primary references

- [llama.cpp server documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
- [llama.cpp speculative decoding](https://github.com/ggml-org/llama.cpp/blob/master/docs/speculative.md)
- [Qwen3.8 official repository](https://github.com/QwenLM/Qwen3.8)
- [Qwen 2.5 Coder](https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct-GGUF)
- [Unsloth Qwen3.8 GGUF](https://huggingface.co/unsloth/Qwen3.8-27B-GGUF)
