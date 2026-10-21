# Model and runtime

## Selected model

Qwen3.8-27B using Unsloth's Dynamic `Q4_K_XL` GGUF quantization. This project downloads the published quant; it does not quantize or train the model on this laptop.

- GGUF: `Qwen3.8-27B-UD-Q4_K_XL.gguf`
- Size: 17,559,178,144 bytes (about 16.35 GiB)
- SHA-256: `3f227079003add2511437e5b1e94812e363385225bf6a9b47b0054a72bc8b01e`
- Optional vision projector: `mmproj-F16.gguf` (not used by the text CLI)
- Projector SHA-256: `cbb841a9ee0636b2ec172f5bb8df2ea8dfeb01e90fe7c6126581d662a0b4e43e`
- Optional speculative MTP draft: `MTP/mtp-Qwen3.8-27B-Q4_0.gguf`, 1,369,590,656 bytes; SHA-256 `50d9ce5a6da381bbcfb31061cf73df94a90e6faf8efeddee379a9cb8f1501c6e`
- Sources: [Qwen model card](https://huggingface.co/Qwen/Qwen3.8-27B), [Unsloth GGUF files](https://huggingface.co/unsloth/Qwen3.8-27B-GGUF), [Unsloth model guidance](https://unsloth.ai/models/qwen3.8-27b)

## Laptop fit

This laptop has 32 GB system RAM and an RTX 4050 Laptop GPU with 6 GB VRAM. The 4-bit model cannot fit entirely in VRAM, so `llama.cpp` places some layers in system memory. The CLI defaults to an 8K context and reserves 1 GiB of GPU memory. Close other memory-heavy applications if loading fails. A smaller context can be selected with `-ContextLength` in `scripts/start-cli.ps1`.

The 4-bit quantization preserves more model capacity than lower-bit options but requires substantial memory. If it does not fit comfortably, reduce the context first. A lower-bit quant may improve memory headroom at some cost to answer quality.

## Runtime and download

- Inference: standalone CUDA-enabled `llama.cpp`; Ollama is not required. The runtime binaries and CUDA DLLs are local-only and ignored by Git; place a compatible build in `runtime/llama.cpp` before starting the agent.
- Agent entry point: `llama-server.exe`, launched by the Python harness and bound to `127.0.0.1` on a temporary port with an API key.
- The server runs with its Web UI disabled, built-in agent tools disabled, and `--offline` set. The harness calls its OpenAI-compatible chat-completions endpoint locally.
- Plain-chat fallback: `llama-completion.exe`, launched by `scripts/start-cli.ps1`; it invokes the local process directly and opens no network listener.
- `scripts/download-model.ps1` downloads the model with resumable parallel ranges using `aria2c` and verifies the SHA-256 checksum. Run it from PowerShell if the model file is missing.
- `scripts/download-model.ps1 -MTP` additionally downloads and verifies the matching MTP draft. Try it with `scripts/start-agent.ps1 -Speculative draft-mtp`; it is not enabled by default because it did not improve measured speed on this laptop.
- Use `-ModelPath` to select another local GGUF. Use `-GPUReserveMiB`, `-KVCacheType`, `-CPUThreads`, and `-BatchThreads` to compare runtime settings. The assistant reports server-side prefill and decode rates after each response.
- The model, cache, and runtime are machine-local and excluded from Git.
