"""Local model discovery, GGUF metadata, and per-model runtime tuning."""

from __future__ import annotations

import mmap
import os
import shutil
import struct
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any


MIN_MODEL_BYTES = 100 * 1024 * 1024
MIB = 1024 * 1024

# Known downloads, in default-preference order: the 27B is the main work model and
# the 7B is a fast fallback that fits entirely in 6 GB of VRAM.
KNOWN_MODELS: dict[str, tuple[str, str]] = {
    "27b": ("Qwen3.8-27B-HF", "Qwen3.8-27B-UD-Q4_K_XL.gguf"),
    "7b": ("Qwen2.5-Coder-7B", "qwen2.5-coder-7b-instruct-q4_k_m.gguf"),
}
MODEL_ALIASES = {
    "27b": "27b", "qwen27b": "27b", "qwen3": "27b", "qwen3.8": "27b", "3.8": "27b",
    "7b": "7b", "coder": "7b", "qwen7b": "7b", "qwen2.5": "7b", "qwen2.5-coder": "7b",
}
REASONING_NAME_TAGS = ("qwen3", "deepseek-r1", "reasoning", "thinking")

# A model counts as GPU-resident when its weights use at most this share of total
# VRAM; the rest is left for the KV cache, compute buffers, and the CUDA context.
GPU_RESIDENT_FRACTION = 0.8


@dataclass(frozen=True)
class ModelInfo:
    path: Path
    name: str
    size_bytes: int
    architecture: str = ""
    context_train: int = 0
    reasoning: bool = False
    mtp: bool = False
    temperature: float = 0.2
    top_p: float = 0.9
    top_k: int = 40

    @property
    def size_gib(self) -> float:
        return self.size_bytes / (1024 ** 3)


@dataclass(frozen=True)
class RuntimeOptions:
    """User overrides from the command line; None means choose automatically."""

    context_length: int | None = None
    cpu_threads: int | None = None
    batch_threads: int | None = None
    gpu_reserve_mib: int = 512
    kv_cache_type: str = "q8_0"
    speculative: str = "auto"
    draft_model: Path | None = None


@dataclass(frozen=True)
class RuntimeTuning:
    context_length: int
    kv_cache_type: str
    batch_size: int
    ubatch_size: int
    gpu_reserve_mib: int
    cpu_threads: int | None
    batch_threads: int | None
    speculative: str
    draft_max: int
    gpu_resident: bool


def known_model_path(project_root: Path, alias: str) -> Path:
    folder, filename = KNOWN_MODELS[alias]
    return project_root / "models" / folder / filename


def discover_models(project_root: Path) -> list[Path]:
    """Return runnable GGUF files under models/, skipping projectors, drafts, and partial files."""
    models_dir = project_root / "models"
    if not models_dir.is_dir():
        return []
    found: list[Path] = []
    for gguf in models_dir.rglob("*.gguf"):
        lowered = gguf.name.lower()
        if "mmproj" in lowered or "mtp" in lowered:
            continue
        try:
            if gguf.stat().st_size < MIN_MODEL_BYTES:
                continue
        except OSError:
            continue
        found.append(gguf.resolve())
    return sorted(found, key=lambda path: path.name.lower())


def default_model(project_root: Path) -> Path | None:
    for alias in KNOWN_MODELS:
        candidate = known_model_path(project_root, alias)
        if candidate.is_file():
            return candidate.resolve()
    discovered = discover_models(project_root)
    return discovered[0] if discovered else None


def resolve_model(target: str | Path, project_root: Path) -> Path | None:
    """Resolve 'auto', a known alias, a path, or a file-name fragment to a GGUF file."""
    text = str(target).strip().strip("'\"")
    lowered = text.lower()
    if lowered in {"", "auto"}:
        return default_model(project_root)
    if lowered in MODEL_ALIASES:
        candidate = known_model_path(project_root, MODEL_ALIASES[lowered])
        return candidate.resolve() if candidate.is_file() else None
    candidate = Path(text).expanduser()
    if candidate.is_file():
        return candidate.resolve()
    for gguf in discover_models(project_root):
        if lowered in gguf.name.lower():
            return gguf
    return None


def model_alias(path: Path, project_root: Path) -> str | None:
    for alias in KNOWN_MODELS:
        try:
            if known_model_path(project_root, alias).resolve() == path.resolve():
                return alias
        except OSError:
            continue
    return None


def display_model_name(model_file: Path) -> str:
    """Readable model name derived from a GGUF file name."""
    stem = model_file.stem
    if "-UD-" in stem:
        return stem.split("-UD-")[0]
    for tag in ("-q4", "-Q4", "-q5", "-Q5", "-q6", "-Q6", "-q8", "-Q8", "-f16", "-F16", "-bf16", "-BF16"):
        if tag in stem:
            return stem.split(tag)[0]
    return stem


_INFO_CACHE: dict[tuple[str, int, int], ModelInfo] = {}


def inspect_model(path: Path) -> ModelInfo:
    """Describe a GGUF model from its embedded metadata, falling back to its file name."""
    path = path.resolve()
    stat = path.stat()
    cache_key = (str(path), stat.st_size, stat.st_mtime_ns)
    cached = _INFO_CACHE.get(cache_key)
    if cached is not None:
        return cached
    try:
        meta = read_gguf_metadata(path)
    except (OSError, ValueError, struct.error):
        meta = {}

    architecture = str(meta.get("general.architecture") or "")
    template = meta.get("tokenizer.chat_template")
    if isinstance(template, str) and template:
        reasoning = "<think>" in template or "enable_thinking" in template
    else:
        reasoning = any(tag in path.name.lower() for tag in REASONING_NAME_TAGS)
    # Qwen recommends temp 1.0 / top-p 0.95 / top-k 20 for thinking; low temperature keeps
    # non-reasoning coder models precise. Sampling defaults embedded in the GGUF win.
    temperature, top_p, top_k = (1.0, 0.95, 20) if reasoning else (0.2, 0.9, 40)
    name = str(meta.get("general.name") or "").strip()
    if name.lower().endswith(" gguf"):
        name = name[:-5].rstrip()
    info = ModelInfo(
        path=path,
        name=name or display_model_name(path),
        size_bytes=stat.st_size,
        architecture=architecture,
        context_train=_as_int(meta.get(f"{architecture}.context_length")),
        reasoning=reasoning,
        mtp=_as_int(meta.get(f"{architecture}.nextn_predict_layers")) > 0,
        temperature=_as_float(meta.get("general.sampling.temp"), temperature),
        top_p=_as_float(meta.get("general.sampling.top_p"), top_p),
        top_k=_as_int(meta.get("general.sampling.top_k")) or top_k,
    )
    _INFO_CACHE[cache_key] = info
    return info


def detect_gpu_memory_mib() -> int | None:
    """Total VRAM of the largest NVIDIA GPU, or None when it cannot be determined."""
    executable = shutil.which("nvidia-smi")
    if not executable:
        return None
    try:
        completed = subprocess.run(
            [executable, "--query-gpu=memory.total", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            timeout=10,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
    except (OSError, subprocess.SubprocessError):
        return None
    values = [int(line.strip()) for line in completed.stdout.splitlines() if line.strip().isdigit()]
    return max(values) if values else None


def plan_runtime(info: ModelInfo, options: RuntimeOptions, vram_mib: int | None) -> RuntimeTuning:
    """Choose llama.cpp settings for a model on this machine.

    Measured on an RTX 4050 Laptop (6 GB) + Ryzen 5 7640HS (6C/12T) + DDR5-5600:
    - Models that spill into system RAM prefill 2.6x faster with 512-token micro-batches,
      because CPU-resident weights are streamed to the GPU once per micro-batch. The larger
      compute buffer costs about 2 GPU layers, roughly 3% generation speed.
    - Their generation is fastest with about two thirds of the logical cores (8 of 12).
    - The built-in MTP head with a 1-token draft gives about +20%; longer drafts verify
      slower on the CPU-resident layers.
    - A q8_0 KV cache plus a 512 MiB reserve keeps the 7B coder entirely in VRAM.
    """
    gpu_resident = vram_mib is not None and info.size_bytes / MIB <= GPU_RESIDENT_FRACTION * vram_mib

    context_length = options.context_length or (8192 if gpu_resident else 16384)
    if info.context_train:
        context_length = min(context_length, info.context_train)

    cpu_threads = options.cpu_threads
    if cpu_threads is None and not gpu_resident:
        cpu_threads = max(1, (os.cpu_count() or 6) * 2 // 3)

    speculative = options.speculative
    has_draft = info.mtp or options.draft_model is not None
    if speculative == "auto":
        speculative = "draft-mtp" if has_draft else "none"
    elif speculative == "draft-mtp" and not has_draft:
        raise ValueError(
            f"{info.name} has no built-in MTP head. Pass --draft-model or choose another --speculative mode."
        )

    return RuntimeTuning(
        context_length=context_length,
        kv_cache_type=options.kv_cache_type,
        batch_size=256 if gpu_resident else 512,
        ubatch_size=128 if gpu_resident else 512,
        gpu_reserve_mib=options.gpu_reserve_mib,
        cpu_threads=cpu_threads,
        batch_threads=options.batch_threads,
        speculative=speculative,
        draft_max=3 if gpu_resident else 1,
        gpu_resident=gpu_resident,
    )


def describe_runtime(info: ModelInfo, tuning: RuntimeTuning) -> str:
    parts = ["fits in VRAM" if tuning.gpu_resident else "GPU + system RAM"]
    if tuning.speculative == "draft-mtp":
        parts.append("MTP speculative decoding")
    elif tuning.speculative != "none":
        parts.append(f"{tuning.speculative} speculation")
    if tuning.cpu_threads:
        parts.append(f"{tuning.cpu_threads} CPU threads")
    parts.append(f"KV {tuning.kv_cache_type}")
    return "  ·  ".join(parts)


# ── GGUF metadata ────────────────────────────────────────────────────

_SCALAR_FORMATS = {0: "B", 1: "b", 2: "H", 3: "h", 4: "I", 5: "i", 6: "f", 7: "?", 10: "Q", 11: "q", 12: "d"}
_STRING = 8
_ARRAY = 9
_MAX_KEYS = 100_000
_MAX_STRING_BYTES = 16 * MIB


def read_gguf_metadata(path: Path) -> dict[str, Any]:
    """Read scalar and string metadata from a GGUF header; arrays such as vocabularies are skipped."""
    with path.open("rb") as handle, mmap.mmap(handle.fileno(), 0, access=mmap.ACCESS_READ) as data:
        if data[:4] != b"GGUF":
            raise ValueError("not a GGUF file")
        (version,) = struct.unpack_from("<I", data, 4)
        if version < 2:
            raise ValueError(f"unsupported GGUF version {version}")
        _tensor_count, key_count = struct.unpack_from("<QQ", data, 8)
        if key_count > _MAX_KEYS:
            raise ValueError("implausible GGUF metadata size")
        position = 24
        metadata: dict[str, Any] = {}
        for _ in range(key_count):
            key, position = _read_string(data, position)
            (kind,) = struct.unpack_from("<I", data, position)
            value, position = _read_value(data, position + 4, kind)
            if value is not None:
                metadata[key] = value
        return metadata


def _read_string(data: Any, position: int) -> tuple[str, int]:
    (length,) = struct.unpack_from("<Q", data, position)
    if length > _MAX_STRING_BYTES:
        raise ValueError("GGUF string is too long")
    start = position + 8
    end = start + length
    if end > len(data):
        raise ValueError("truncated GGUF metadata")
    return bytes(data[start:end]).decode("utf-8", errors="replace"), end


def _read_value(data: Any, position: int, kind: int) -> tuple[Any, int]:
    if kind in _SCALAR_FORMATS:
        fmt = "<" + _SCALAR_FORMATS[kind]
        return struct.unpack_from(fmt, data, position)[0], position + struct.calcsize(fmt)
    if kind == _STRING:
        return _read_string(data, position)
    if kind == _ARRAY:
        return None, _skip_array(data, position)
    raise ValueError(f"unsupported GGUF value type {kind}")


def _skip_array(data: Any, position: int) -> int:
    item_kind, count = struct.unpack_from("<IQ", data, position)
    position += 12
    if item_kind in _SCALAR_FORMATS:
        position += struct.calcsize("<" + _SCALAR_FORMATS[item_kind]) * count
    elif item_kind == _STRING:
        for _ in range(count):
            (length,) = struct.unpack_from("<Q", data, position)
            position += 8 + length
    elif item_kind == _ARRAY:
        for _ in range(count):
            position = _skip_array(data, position)
    else:
        raise ValueError(f"unsupported GGUF array type {item_kind}")
    if position > len(data):
        raise ValueError("truncated GGUF metadata")
    return position


def _as_int(value: Any) -> int:
    try:
        return int(value) if value is not None and not isinstance(value, bool) else 0
    except (TypeError, ValueError):
        return 0


def _as_float(value: Any, default: float) -> float:
    try:
        return float(value) if value is not None and not isinstance(value, bool) else default
    except (TypeError, ValueError):
        return default
