# Luma

A local coding agent for Windows, written in Rust. Luma streams answers from a GGUF model through llama.cpp, reads and edits project files, and runs PowerShell commands with the permissions you choose. Web research starts off. Your project, prompts, and saved conversations stay on your machine during local tasks.

## Build and launch

Install stable Rust with the Windows MSVC toolchain, Visual Studio C++ Build Tools, and the Windows SDK. Cargo builds the terminal application and bundles SQLite into its executable. The first build downloads the locked crate dependencies. [Rust installation](https://www.rust-lang.org/tools/install)

```powershell
git clone https://github.com/zhengbingquant/local-desktop-assistant.git
cd local-desktop-assistant
cargo build --release --locked
```

**Upgrading an existing checkout?** Build once after pulling, then restart Luma. Existing model files, runtime installations, command shims, and the SQLite chat database work with the Rust version.

For a fresh installation, run the setup commands from the checkout. The runtime installer defaults to CUDA 13. Choose `--backend vulkan` or `--backend cpu` for a machine without an NVIDIA GPU.

```powershell
.\target\release\luma.exe install-runtime
.\target\release\luma.exe download 7b
.\target\release\luma.exe install-command

# In a new terminal
cd C:\path\to\project
luma
```

You can also launch `.\target\release\luma.exe --workspace C:\path\to\project` directly, or drop a project folder onto **Start Assistant.bat**. The existing `scripts\*.ps1` entry points forward to the same executable. Once the runtime, weights and binary are installed, starting Luma requires no network access.

## Working in the terminal

Type `/` at the prompt to open the command palette. Keep typing to narrow the list, move with **↑ / ↓**, then press **Enter** to run a command. **Tab** completes it so you can add arguments. **Esc** dismisses the list. Model, mode and conversation pickers also support typing to filter.

```text
❯ /mo
 › /mode         Choose how the agent can act
   /model        Select a local GGUF model
─────────────────────────────────────────────────────────
 Ask  ·  Qwen2.5 Coder 7B  ·  8% context  ·  web off
```

**Shift+Tab** cycles through Ask, Accept edits, and Plan while preserving your draft. The footer shows the active mode, model, context usage, and web setting. **Alt+Enter** inserts a line break. **Alt+V** pastes clipboard text as a single draft on Windows. **Ctrl+C** interrupts a generation or clears the current input. Press it at an empty prompt to exit.

| Mode | Project reads | File changes | PowerShell |
| --- | --- | --- | --- |
| **Ask** (default) | Allowed | Show diff and ask | Show command and ask |
| **Accept edits** (`/mode workspace`) | Allowed | Apply the displayed diff | Show command and ask |
| **Plan** (`/plan`) | Allowed | Blocked | Blocked |
| **Full access** (`/mode full`) | Allowed | Apply the displayed diff | Run the displayed command |

Full access is an explicit menu or command choice. File tools stay inside the workspace in every mode. PowerShell runs with the permissions and network access of your Windows account. Switching project folders restores Ask mode.

### Resume a conversation

Launch Luma in the same folder to reopen its active chat. For a different conversation, type `/resume` and select it from the searchable list. The model server starts on the first request, so you can browse history before loading weights.

- `/resume list` prints a numbered list. `/resume 2` continues the second entry in its original workspace.
- If a project moved, open its new folder and use `/resume 2 --here` to continue a separate copy there.
- `/new` or `/clear` starts fresh and keeps earlier chats available.
- Sessions live in `%LOCALAPPDATA%\LocalAssistant\sessions\history.sqlite3`. Earlier profile JSON chats and archives import at startup. A workspace-local `.local_assistant\chat-session.json` imports when you open that project.

### Commands

| Command | Action |
| --- | --- |
| `/mode`, `/permissions` | Choose a permission mode |
| `/plan` | Switch to read-only planning |
| `/model`, `/model 7b` | Pick or switch local weights |
| `/effort`, `/effort budget 512` | Set reasoning depth or thinking-token budget |
| `/think` | Show or hide streamed reasoning |
| `/web auto`, `/web off` | Enable or disable DuckDuckGo queries and public HTTPS page fetches |
| `/workspace [path]` | Show or change project folder |
| `/context`, `/compact` | Inspect token usage or summarize older turns |
| `/history`, `/history all` | Show recent messages or the full saved history |
| `/save`, `/resume`, `/new` | Save, select, or start a conversation |
| `/delete`, `/delete all` | Confirm deletion of this workspace’s chats or all saved chats |
| `/cls`, `/help`, `/exit` | Clear the screen, list commands, or quit |

## Models and performance

| Download | Size | Typical use |
| --- | ---: | --- |
| `luma download 7b` | 4.4 GiB | Qwen 2.5 Coder 7B for quick answers and small edits |
| `luma download 27b` | 16.4 GiB | Qwen3.8-27B for multi-step coding, with 32 GB system RAM suggested |

Downloads resume from a partial file and verify SHA-256 before exposing the finished GGUF to the agent. `auto` prefers the bundled 27B when available. Put other GGUFs under `models\` or pass `--model C:\path\to\model.gguf`.

Runtime defaults depend on GGUF metadata and available GPU memory. Earlier measurements on an RTX 4050 laptop recorded about **37 tokens/s** for the GPU-resident 7B and **2.9 tokens/s** for the 27B split across GPU and system RAM. Those figures describe llama.cpp tuning on that machine. The Rust port has no new inference benchmark. [Model setup and measurements](docs/model-setup.md)

```powershell
luma --model 7b --context-length 8192
luma --model 27b --reasoning-effort low --gpu-reserve-mib 1024
luma chat --model 7b                 # Plain offline chat without workspace tools
luma install-runtime --backend cuda-12 --force
luma --help                          # All native flags and setup commands
```

The original launcher option names, including `-WorkspacePath`, `-ModelPath`, `-Model`, `-EnableWeb` and `-ThinkingBudget`, remain available. Set `LUMA_HOME` or pass `--home` if the executable lives separately from the installation’s `runtime\` and `models\` directories.

## Privacy and troubleshooting

- The inference server listens on loopback with a per-run key. Luma disables proxies, redirects, the built-in agent, and the web UI for inference. Windows Job Objects clean up the server and shell processes when Luma closes.
- Web research requires an explicit opt-in. When enabled, the terminal displays each query or URL. Plan mode exposes only project inspection tools.
- File tools skip linked paths, common secrets, binary files, and root `.gitignore` matches. Session database backups contain readable transcripts.
- If the launcher asks for a binary, rebuild from the checkout with `cargo build --release --locked`. If the linker or C compiler is missing, install the Visual Studio **Desktop development with C++** workload.
- If weights fail to load, choose the 7B, reduce `--context-length` or raise `--gpu-reserve-mib`. Missing installations can be restored with `luma install-runtime` and `luma download 7b`.

[Agent architecture and design decisions](docs/agent-harness.md) · [MIT license](LICENSE)
