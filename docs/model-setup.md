# Model and runtime

## How models are detected

The assistant reads each GGUF file's embedded metadata (`local_assistant/models.py`, standard library only) instead of guessing from file names:

| Metadata | Used for |
| :-- | :-- |
| `tokenizer.chat_template` contains `<think>` / `enable_thinking` | Reasoning on/off, sampling defaults, reasoning-effort request fields |
| `<arch>.nextn_predict_layers > 0` | A built-in MTP head is available for speculative decoding |
| `<arch>.context_length` | Upper bound for the context window |
| `general.sampling.*` | Recommended temperature / top-p / top-k when the GGUF ships them |
| `general.name` | Display name and the model identity given to the assistant |

Any other GGUF placed under `models/` shows up in `/model` and can be selected with `/model <part of the file name>` or `-ModelPath`.

## Supported models

### Qwen3.8-27B (default)

- **GGUF**: `Qwen3.8-27B-UD-Q4_K_XL.gguf`, 17,559,178,144 bytes (16.35 GiB), from [Unsloth](https://huggingface.co/unsloth/Qwen3.8-27B-GGUF)
- **Download**: `.\scripts\download-model.ps1`
- **Architecture**: `qwen35` hybrid. Gated DeltaNet linear-attention layers with full attention every fourth layer, so only 16 of 64 layers keep a KV cache. One built-in MTP (multi-token prediction) layer.
- **License**: Apache 2.0 (per the GGUF metadata)

It does not fit in 6 GB of VRAM: about 12 of its 65 layers run on the GPU and the rest stream from system RAM, so generation is bound by memory bandwidth. The optional `-MTP` download (a separate 1.3 GB draft file) is not needed with the bundled llama.cpp build, which uses the MTP head inside the main GGUF.

### Qwen 2.5 Coder 7B Instruct (fast fallback)

- **GGUF**: `qwen2.5-coder-7b-instruct-q4_k_m.gguf`, 4,683,073,536 bytes (4.36 GiB), from [Qwen/Qwen2.5-Coder-7B-Instruct-GGUF](https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct-GGUF)
- **Download**: `.\scripts\download-coder-7b.ps1` (SHA-256 verified)
- **License**: Apache 2.0

With a q8_0 KV cache and a 512 MiB VRAM reserve it fits entirely on the GPU at 8K context. It is fast but much weaker at multi-step agent work than the 27B.

## Tuning for this laptop

**Machine**: AMD Ryzen 5 7640HS (6 cores / 12 threads, Zen 4) · 32 GB DDR5-5600 dual channel · NVIDIA RTX 4050 Laptop GPU, 6 GB, PCIe 4.0 x8 · Windows 11 · llama.cpp b11748 (CUDA 13.4 build).

`plan_runtime()` in `local_assistant/models.py` picks settings per model. A model counts as *GPU-resident* when its weights take at most 80% of total VRAM.

| Setting | Spills into RAM (27B) | GPU-resident (7B) | Old default |
| :-- | :-- | :-- | :-- |
| Context | 16K | 8K | 8K |
| KV cache | q8_0 | q8_0 | f16 |
| VRAM reserve (`--fit-target`) | 512 MiB | 512 MiB | 1024 MiB |
| Batch / micro-batch | 512 / 512 | 256 / 128 | 256 / 128 |
| CPU threads | ⅔ of logical cores (8) | llama.cpp default | 6 for the 27B |
| Speculative decoding | built-in MTP, 1-token draft | none (no MTP head) | none |

### Results

Measured with the agent's real system prompt and tool schemas through `llama-server`.

#### Qwen3.8-27B

| | Before | After |
| :-- | --: | --: |
| First request (2,281 prompt tokens) | 35.3 s (64.6 tok/s) | 15.8 s (145 tok/s) |
| Each tool round (~750–880 new tokens) | 12–14 s | 6.0 s |
| Generation | 2.04 tok/s | 2.94 tok/s |
| Context window | 8K | 16K |

#### Qwen 2.5 Coder 7B

| | Before | After |
| :-- | --: | --: |
| GPU layers | 24 of 29 | 29 of 29 |
| Prefill | 824 tok/s | 1,756 tok/s |
| Generation | 19.0 tok/s | 36.8 tok/s |

### Experiments behind the choices (Qwen3.8-27B)

- **CPU threads** (`llama-bench`, 14 GPU layers): 6 → 2.24, 7 → 2.38, **8 → 2.54**, 10 → 2.47, 12 → 2.38 tok/s. Decode is memory-bound; a few SMT threads help, all 12 contend.
- **Micro-batch vs. prefill** (10 GPU layers): ub128 → 71, ub256 → 125, **ub512 → 182**, ub1024 → 266 tok/s. Weights that live in system RAM are copied to the GPU once per micro-batch, so bigger micro-batches amortize the PCIe transfer.
- **Micro-batch vs. GPU layers** (512 MiB reserve): ub128 → 14 layers, ub512 → 12, ub1024 → 10, ub2048 → 6. Generation at 14 / 12 / 10 layers: 2.54 / 2.47 / 2.38 tok/s.
- **MTP draft length** (256 tokens of code and reasoning): no speculation 2.42, **1 token 2.94** (95% accepted), 3 tokens 2.75 (76% accepted) tok/s. Verifying several draft tokens on the CPU-resident layers is compute-bound on six cores.
- **Context length**: 8K, 16K, and 32K fit the same number of GPU layers. The hybrid model's KV cache is small and mostly sits with the CPU-resident layers.
- **Prompt cache**: llama.cpp's default context checkpoints already reuse the cache across agent rounds on this hybrid model (only the new tokens are processed), so no checkpoint tuning is needed.

### Trade-offs

- **ub512 over ub1024**: 1024 prefills 46% faster but costs two more GPU layers (about 4% slower generation). Generation dominates a typical 27B agent turn, so 512 is the better balance.
- **512 MiB VRAM reserve** instead of llama.cpp's 1 GiB: buys two GPU layers. On this laptop the RTX 4050 drives no display and idles at about 300 MiB used. If another program needs VRAM, launch with `-GPUReserveMiB 1024`.
- **q8_0 KV cache**: halves KV memory with negligible quality loss. Compare with `-KVCacheType f16`.
- **16K context for the 27B**: costs no GPU layers and about 0.5 GiB of RAM, and halves how often older turns must be summarized.
- **⅔-of-cores heuristic**: measured on one 6-core SMT CPU. Override with `-CPUThreads`.

## Runtime and download

- **Inference**: standalone `llama.cpp`; Ollama is not required. `.\scripts\install-runtime.ps1` downloads the tested build (b11748, CUDA 13) into `runtime\llama.cpp` and checks the SHA-256 digests GitHub publishes. Use `-Backend cuda-12` for older NVIDIA drivers, `-Backend vulkan` or `-Backend cpu` without an NVIDIA GPU, and `-Tag latest -Force` to upgrade.
- **Entry point**: `llama-server.exe`, launched by the Python harness, bound to `127.0.0.1` on a temporary port with a per-session API key.
- **Server config**: Web UI disabled, built-in agent tools disabled, `--offline` set. The harness calls the OpenAI-compatible chat-completions endpoint locally.
- **Plain chat**: `.\scripts\start-cli.ps1 [-Model auto|27b|7b]` gives a direct chat without workspace tools.
- Models, caches, and runtime binaries are machine-local and excluded from Git.
