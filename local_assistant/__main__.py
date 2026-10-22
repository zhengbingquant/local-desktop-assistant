"""Command-line entry point for the local assistant."""

from __future__ import annotations

import argparse
from pathlib import Path

from .agent import launch_agent


def main() -> int:
    project_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description="Run a local Qwen coding assistant with approval-gated tools.")
    coder_7b_file = project_root / "models" / "Qwen2.5-Coder-7B" / "qwen2.5-coder-7b-instruct-q4_k_m.gguf"
    default_27b_file = project_root / "models" / "Qwen3.8-27B-HF" / "Qwen3.8-27B-UD-Q4_K_XL.gguf"
    default_model_file = coder_7b_file if coder_7b_file.is_file() else default_27b_file
    parser.add_argument(
        "--workspace",
        type=Path,
        default=Path.cwd(),
        help="folder the assistant may inspect and edit (default: current folder)",
    )
    parser.add_argument(
        "--context-length",
        type=int,
        default=8192,
        help="llama.cpp context length; 8192 is the laptop-friendly default",
    )
    parser.add_argument("--model", type=Path, default=default_model_file, help="local GGUF model path")
    web_group = parser.add_mutually_exclusive_group()
    web_group.add_argument(
        "--web-mode",
        choices=("auto", "off"),
        default="auto",
        help="let the model choose relevant public web lookups, or disable web access (default: auto)",
    )
    web_group.add_argument("--enable-web", dest="web_mode", action="store_const", const="auto", help=argparse.SUPPRESS)
    web_group.add_argument("--disable-web", dest="web_mode", action="store_const", const="off", help="disable all web tools")
    parser.add_argument("--cpu-threads", type=int, help="CPU threads for token generation (omit for llama.cpp auto)")
    parser.add_argument("--batch-threads", type=int, help="CPU threads for prompt processing (omit to match generation threads)")
    parser.add_argument("--gpu-reserve-mib", type=int, default=1024, help="VRAM to leave free when auto-fitting GPU layers (default: 1024)")
    parser.add_argument(
        "--kv-cache-type",
        choices=("f16", "q8_0"),
        default="f16",
        help="KV-cache precision; q8_0 uses less memory, f16 is the fidelity default",
    )
    parser.add_argument(
        "--speculative",
        choices=("auto", "none", "ngram-mod", "ngram-simple", "draft-mtp"),
        default="none",
        help="optional speculative decoder; none is the measured default",
    )
    parser.add_argument("--draft-model", type=Path, help="draft model file required by draft-mtp")
    parser.add_argument(
        "--reasoning-effort",
        choices=("none", "low", "medium", "high"),
        default="medium",
        help="reasoning depth; lower settings reduce latency on routine tasks",
    )
    parser.add_argument(
        "--thinking-budget",
        type=int,
        default=512,
        help="maximum reasoning tokens per model response (default: 512; 0 disables the budget)",
    )
    parser.add_argument(
        "--thinking-display",
        choices=("show", "hide"),
        default="show",
        help="display or hide reasoning tokens in the terminal (default: show)",
    )
    args = parser.parse_args()

    workspace = args.workspace.expanduser().resolve()
    if not workspace.is_dir():
        parser.error(f"workspace is not an existing directory: {workspace}")
    if args.context_length < 4096 or args.context_length > 32768:
        parser.error("--context-length must be between 4096 and 32768")
    if args.cpu_threads is not None and not 1 <= args.cpu_threads <= 24:
        parser.error("--cpu-threads must be between 1 and 24")
    if args.batch_threads is not None and not 1 <= args.batch_threads <= 24:
        parser.error("--batch-threads must be between 1 and 24")
    if not 128 <= args.gpu_reserve_mib <= 4096:
        parser.error("--gpu-reserve-mib must be between 128 and 4096")
    if not 0 <= args.thinking_budget <= 8192:
        parser.error("--thinking-budget must be between 0 and 8192")

    server_exe = project_root / "runtime" / "llama.cpp" / "llama-server.exe"
    model_file = args.model.expanduser().resolve()
    if not model_file.is_file():
        parser.error(f"model file does not exist: {model_file}")
    default_draft_model = project_root / "models" / "Qwen3.8-27B-HF" / "MTP" / "mtp-Qwen3.8-27B-Q4_0.gguf"
    draft_model_file = args.draft_model.expanduser().resolve() if args.draft_model else None
    if args.speculative == "auto":
        draft_model_file = draft_model_file or (default_draft_model if default_draft_model.is_file() else None)
        speculative = "draft-mtp" if draft_model_file else "none"
    else:
        speculative = args.speculative
    if speculative == "draft-mtp":
        draft_model_file = draft_model_file or default_draft_model
        if not draft_model_file.is_file():
            parser.error("MTP draft file is missing; run .\\scripts\\download-model.ps1 -MTP")
    elif draft_model_file is not None:
        parser.error("--draft-model can only be used with --speculative draft-mtp")
    return launch_agent(
        workspace,
        server_exe,
        model_file,
        args.context_length,
        args.web_mode == "auto",
        cpu_threads=args.cpu_threads,
        batch_threads=args.batch_threads,
        gpu_reserve_mib=args.gpu_reserve_mib,
        kv_cache_type=args.kv_cache_type,
        speculative=speculative,
        reasoning_effort=args.reasoning_effort,
        thinking_budget_tokens=args.thinking_budget,
        draft_model_file=draft_model_file,
        thinking_display=args.thinking_display,
    )


if __name__ == "__main__":
    raise SystemExit(main())
