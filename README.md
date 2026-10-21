# Local Assistant

A local coding-agent CLI for Qwen3.8-27B. The model runs through a local CUDA-enabled build of `llama.cpp`; a small Python controller supplies workspace tools and mediates writes and commands. No cloud model or Ollama is required.

![Local Assistant CLI](docs/assets/local-assistant-terminal.png)

## Start

- Install a terminal command once: `powershell -ExecutionPolicy Bypass -File .\scripts\install-command.ps1`. Reopen the terminal, then type `luma` (or `local-assistant`) from the folder you want the assistant to work in.
- `luma "C:\path\to\project"` starts it with that folder as the workspace. Add `-DisableWeb` to run with web tools disabled.
- Double-click **Start Assistant.bat** to work in this project folder.
- To work in another project, run `Start Assistant.bat "C:\path\to\project"`.
- Or open PowerShell and run `.\scripts\start-agent.ps1 -WorkspacePath "C:\path\to\project"`.

The agent needs Python 3.11 or newer. It uses only the Python standard library. The model loads when the agent starts and may take several minutes. The default context is 8K; use `-ContextLength 4096` if memory is tight. Type `/help` for CLI commands and `/exit` to stop. The CLI prints prompt and generation speed after each model response.

If the model file is missing, install `aria2` and run `.\scripts\download-model.ps1`.

The `models/` and `runtime/` folders are intentionally excluded from Git. To run a fresh clone, provide a compatible CUDA-enabled `llama.cpp` build under `runtime/llama.cpp` (including its required DLLs), then download the model with the script above. The current project launcher expects `llama-server.exe` there.

The model path and runtime tuning are configurable. For example, `-ModelPath "D:\models\other.gguf"` selects another local GGUF; `-ReasoningEffort none` and `-ThinkingBudget 0` reduce deliberation for routine requests. During a session, use `/effort none|low|medium|high` or `/effort budget N` (0–8192) to tune them together.

At launch, the CLI shows a large LOCAL AI wordmark, a greeting, and the active workspace/model settings before loading the model. Type `/` to browse slash commands; use the arrow keys to choose one, Tab to complete it, and Enter to run it. On terminals without interactive key support, type `/` and press Enter to print the command list.

## What it can do

- Inspect a workspace with `list_files`, `read_file`, and literal `search_text`.
- Propose file creation, replacement, and deletion. The CLI prints a diff; approval behavior follows the active `/permission` mode.
- Propose PowerShell commands. The full command and working directory are shown; whether they need approval depends on `/permission`.
- Automatically search and read public web pages when you ask for a lookup, need current facts, or the model cannot answer reliably. Search queries go to DuckDuckGo and each lookup is shown in the terminal. Use `/web off` or launch with `-DisableWeb` for a fully offline session; `/web on` re-enables lookups.
- Reset the conversation with `/new` (or `/clear`), inspect the active workspace with `/workspace`, adjust reasoning with `/effort`, and set tool approvals with `/permission`.

`/permission ask` is the default and asks before every file change, deletion, and shell command. `/permission workspace` applies workspace file changes after showing their diff, while shell commands still ask. `/permission full` skips per-action approval for file changes and shell commands for the current session; shell commands run as your Windows account and can affect files outside the workspace. Use `/permission ask` to restore prompts. File tools themselves stay inside the selected workspace, skip symlinks and common generated folders, and block common secret files. Fetched files and web pages are untrusted data and cannot change the active permission mode.

The inference server listens only on `127.0.0.1` using a temporary port and API key. It runs in offline mode, the bundled web UI and built-in server agent tools are disabled, and Windows stops the model server and assistant-run commands when the CLI exits or its window is closed. Conversation history is kept in memory for that session only.

## Model

- Qwen3.8-27B, Unsloth Dynamic `Q4_K_XL` GGUF, about 16.35 GiB.
- RTX 4050 Laptop GPU with 6 GB VRAM, 32 GB RAM, Ryzen 5 7640HS.
- The model uses both GPU and system memory; a local measurement produced about 2.1 tokens/second. That rate is a hardware/model limit, not something a different agent loop can remove. See [model and runtime notes](docs/model-setup.md).
- The optional 1.37 GB Qwen MTP draft can be downloaded with `.\scripts\download-model.ps1 -MTP` and tried with `-Speculative draft-mtp`. It is off by default because the local comparison did not show a speed gain.

## Backend notes

The agent loop, workspace tools, approval rules, and CLI are our own Python code. `llama.cpp` is the optimized local inference engine; the Python controller owns the available tool list and validates calls. Qwen's examples name a Qwen-specific parser for vLLM and SGLang, so exact tool-call behavior can differ with the GGUF template and llama.cpp build. The controller also parses Qwen's textual XML-style `<tool_call>` form, but model tool selection still needs real-world evaluation. Performance controls and measurements are in [agent-harness.md](docs/agent-harness.md).

For plain chat without workspace tools, `.\scripts\start-cli.ps1` remains available.
