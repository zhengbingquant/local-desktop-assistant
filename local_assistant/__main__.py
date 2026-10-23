"""Command-line entry point for the local assistant."""

from __future__ import annotations

import argparse
import os
from pathlib import Path

from .agent import launch_agent
from .models import RuntimeOptions, resolve_model


def main() -> int:
    project_root = Path(__file__).resolve().parents[1]
    max_threads = max(1, os.cpu_count() or 1)
    parser = argparse.ArgumentParser(
        prog="local_assistant",
        description="Run a private, local coding assistant with approval-gated tools.",
    )
    parser.add_argument(
        "--workspace",
        type=Path,
        default=Path.cwd(),
        help="folder the assistant may inspect and edit (default: current folder)",
    )
    parser.add_argument(
        "--model",
        default="auto",
        help="'auto' (Qwen3.8-27B when downloaded, else Qwen 2.5 Coder 7B), '27b', '7b', "
        "part of a file name under models/, or a path to a GGUF file",
    )
    parser.add_argument(
        "--context-length",
        type=int,
        help="context window in tokens (default: 16384 when the model spills into system RAM, "
        "8192 when it fits in VRAM)",
    )
    web_group = parser.add_mutually_exclusive_group()
    web_group.add_argument(
        "--web-mode",
        choices=("auto", "off"),
        default="auto",
        help="let the model choose relevant public web lookups, or disable web access (default: auto)",
    )
    web_group.add_argument("--enable-web", dest="web_mode", action="store_const", const="auto", help=argparse.SUPPRESS)
    web_group.add_argument("--disable-web", dest="web_mode", action="store_const", const="off", help="disable all web tools")
    parser.add_argument(
        "--cpu-threads",
        type=int,
        help="CPU threads for token generation (default: two thirds of logical cores when the model "
        "spills into system RAM, otherwise llama.cpp's choice)",
    )
    parser.add_argument("--batch-threads", type=int, help="CPU threads for prompt processing (default: same as generation)")
    parser.add_argument(
        "--gpu-reserve-mib",
        type=int,
        default=512,
        help="VRAM to leave free when fitting layers on the GPU (default: 512)",
    )
    parser.add_argument(
        "--kv-cache-type",
        choices=("q8_0", "f16"),
        default="q8_0",
        help="KV-cache precision; q8_0 halves cache memory with negligible quality loss (default: q8_0)",
    )
    parser.add_argument(
        "--speculative",
        choices=("auto", "none", "draft-mtp", "ngram-mod", "ngram-simple"),
        default="auto",
        help="speculative decoding; auto uses the model's built-in MTP head when it has one (default: auto)",
    )
    parser.add_argument(
        "--draft-model",
        type=Path,
        help="separate draft GGUF for --speculative draft-mtp (not needed for models with a built-in MTP head)",
    )
    parser.add_argument(
        "--reasoning-effort",
        choices=("none", "low", "medium", "high"),
        default="medium",
        help="reasoning depth for thinking models; none disables thinking (default: medium)",
    )
    parser.add_argument(
        "--thinking-budget",
        type=int,
        default=512,
        help="maximum reasoning tokens per model response (default: 512; 0 = no limit)",
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
    if args.context_length is not None and not 2048 <= args.context_length <= 262144:
        parser.error("--context-length must be between 2048 and 262144")
    if args.cpu_threads is not None and not 1 <= args.cpu_threads <= max_threads:
        parser.error(f"--cpu-threads must be between 1 and {max_threads}")
    if args.batch_threads is not None and not 1 <= args.batch_threads <= max_threads:
        parser.error(f"--batch-threads must be between 1 and {max_threads}")
    if not 128 <= args.gpu_reserve_mib <= 4096:
        parser.error("--gpu-reserve-mib must be between 128 and 4096")
    if not 0 <= args.thinking_budget <= 8192:
        parser.error("--thinking-budget must be between 0 and 8192")

    model_file = resolve_model(args.model, project_root)
    if model_file is None:
        parser.error(
            f"model not found: {args.model}. Download one with .\\scripts\\download-model.ps1 "
            "or pass a path to a GGUF file."
        )
    draft_model = args.draft_model.expanduser().resolve() if args.draft_model else None
    if draft_model is not None:
        if not draft_model.is_file():
            parser.error(f"draft model does not exist: {draft_model}")
        if args.speculative not in {"auto", "draft-mtp"}:
            parser.error("--draft-model can only be used with --speculative auto or draft-mtp")

    options = RuntimeOptions(
        context_length=args.context_length,
        cpu_threads=args.cpu_threads,
        batch_threads=args.batch_threads,
        gpu_reserve_mib=args.gpu_reserve_mib,
        kv_cache_type=args.kv_cache_type,
        speculative=args.speculative,
        draft_model=draft_model,
    )
    return launch_agent(
        workspace,
        project_root / "runtime" / "llama.cpp" / "llama-server.exe",
        model_file,
        options,
        args.web_mode == "auto",
        reasoning_effort=args.reasoning_effort,
        thinking_budget_tokens=args.thinking_budget,
        thinking_display=args.thinking_display,
    )


if __name__ == "__main__":
    raise SystemExit(main())
