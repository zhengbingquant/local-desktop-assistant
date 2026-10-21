# Model and runtime

## Supported models

The assistant auto-detects the loaded model and adjusts inference parameters accordingly. Reasoning models (Qwen3.8, DeepSeek-R1) get `--reasoning on` with thinking budgets; non-reasoning coder models get `--reasoning off` with lower temperature for precise code generation.

### Qwen 2.5 Coder 7B Instruct (recommended default)

- **GGUF**: `qwen2.5-coder-7b-instruct-q4_k_m.gguf`
- **Size**: 4,683,073,536 bytes (about 4.4 GiB)
- **Source**: [Qwen/Qwen2.5-Coder-7B-Instruct-GGUF](https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct-GGUF)
- **Download**: `.\scripts\download-coder-7b.ps1`

This model fits entirely in the RTX 4050's 6 GB VRAM, producing **40–55 tokens/second** generation speed. It is trained specifically on code and tool-calling tasks, making it the best choice for an iterative coding agent that needs fast turnaround.

### Qwen3.8-27B (optional, deeper reasoning)

- **GGUF**: `Qwen3.8-27B-UD-Q4_K_XL.gguf`
- **Size**: 17,559,178,144 bytes (about 16.35 GiB)
- **Source**: [Unsloth Qwen3.8 GGUF](https://huggingface.co/unsloth/Qwen3.8-27B-GGUF)
- **Download**: `.\scripts\download-model.ps1`
- **Optional MTP draft**: `MTP/mtp-Qwen3.8-27B-Q4_0.gguf`, 1,369,590,656 bytes. Download with `.\scripts\download-model.ps1 -MTP` and try with `-Speculative draft-mtp`.

This 27B model cannot fit entirely in 6 GB VRAM. Most layers are offloaded to system RAM, producing about **2 tokens/second**. Suitable for complex reasoning tasks where speed is less important, but too slow for iterative agent workflows.

## Laptop fit

This laptop has 32 GB system RAM and an RTX 4050 Laptop GPU with 6 GB VRAM. The 7B coder model fits entirely in VRAM; the 27B model requires CPU offloading. The CLI defaults to an 8K context and reserves 1 GiB of GPU memory. Close other memory-heavy applications if loading fails. Use `-ContextLength 4096` if memory is tight.

## Runtime and download

- **Inference**: standalone CUDA-enabled `llama.cpp`; Ollama is not required. Place a compatible build under `runtime/llama.cpp` (including its required DLLs).
- **Entry point**: `llama-server.exe`, launched by the Python harness, bound to `127.0.0.1` on a temporary port with an API key.
- **Server config**: Web UI disabled, built-in agent tools disabled, `--offline` set. The harness calls the OpenAI-compatible chat-completions endpoint locally.
- **Plain chat**: `.\scripts\start-cli.ps1` provides direct chat without workspace tools.
- Models, cache, and runtime binaries are machine-local and excluded from Git.
