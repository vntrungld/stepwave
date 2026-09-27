"""`.swm` v1: b"SWM1", u32 LE version, u32 LE header length, UTF-8 JSON header, then f32 LE
tensors concatenated in header order (no padding). Read by `core` in M4."""

from __future__ import annotations

import json
import struct
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

from . import NUM_BANDS
from .errors import TrainerError
from .model import DENSE, HIDDEN, INPUTS

MAGIC = b"SWM1"
VERSION = 1
ARCH = "dense-gru2-dense"
_PREFIX = 12  # magic + version + header length


def tensor_layout() -> list[tuple[str, tuple[int, ...]]]:
    """Payload order: PyTorch state_dict names and shapes; GRU gates stacked r, z, n."""
    g = 3 * HIDDEN
    return [
        ("inp.weight", (DENSE, INPUTS)),
        ("inp.bias", (DENSE,)),
        ("gru1.weight_ih_l0", (g, DENSE)),
        ("gru1.weight_hh_l0", (g, HIDDEN)),
        ("gru1.bias_ih_l0", (g,)),
        ("gru1.bias_hh_l0", (g,)),
        ("gru2.weight_ih_l0", (g, HIDDEN)),
        ("gru2.weight_hh_l0", (g, HIDDEN)),
        ("gru2.bias_ih_l0", (g,)),
        ("gru2.bias_hh_l0", (g,)),
        ("out.weight", (NUM_BANDS, HIDDEN)),
        ("out.bias", (NUM_BANDS,)),
    ]


@dataclass(frozen=True)
class SwmModel:
    header: dict[str, Any]
    tensors: dict[str, np.ndarray]


def write_swm(path: Path, header: dict[str, Any], tensors: dict[str, np.ndarray]) -> None:
    layout = tensor_layout()
    if set(tensors) != {n for n, _ in layout}:
        raise TrainerError("tensor names do not match the swm v1 layout")
    payload = []
    for name, shape in layout:
        t = np.asarray(tensors[name], dtype="<f4")
        if t.shape != shape:
            raise TrainerError(f"tensor {name}: shape {t.shape}, expected {shape}")
        if not np.all(np.isfinite(t)):
            raise TrainerError(f"tensor {name} has non-finite values")
        payload.append(t.tobytes())
    full = {**header, "tensors": [{"name": n, "shape": list(s)} for n, s in layout]}
    head = json.dumps(full, sort_keys=True).encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(MAGIC + struct.pack("<II", VERSION, len(head)) + head + b"".join(payload))


def read_swm(path: Path) -> SwmModel:
    data = path.read_bytes()
    if data[:4] != MAGIC:
        raise TrainerError(f"{path}: bad magic {data[:4]!r}, expected {MAGIC!r}")
    if len(data) < _PREFIX:
        raise TrainerError(f"{path}: truncated before the header")
    version, head_len = struct.unpack_from("<II", data, 4)
    if version != VERSION:
        raise TrainerError(f"{path}: format version {version}, expected {VERSION}")
    try:
        header = json.loads(data[_PREFIX : _PREFIX + head_len].decode())
    except (UnicodeDecodeError, json.JSONDecodeError) as err:
        raise TrainerError(f"{path}: header is not valid JSON (truncated?): {err}") from err
    if header.get("arch") != ARCH:
        raise TrainerError(f"{path}: arch {header.get('arch')!r}, expected {ARCH!r}")
    specs = [(d["name"], tuple(d["shape"])) for d in header.get("tensors", [])]
    if specs != tensor_layout():
        raise TrainerError(f"{path}: tensor list does not match the swm v1 layout")
    tensors, pos = {}, _PREFIX + head_len
    for name, shape in specs:
        count = int(np.prod(shape))
        if pos + 4 * count > len(data):
            raise TrainerError(f"{path}: payload truncated at tensor {name}")
        t = np.frombuffer(data, dtype="<f4", count=count, offset=pos).reshape(shape)
        if not np.all(np.isfinite(t)):
            raise TrainerError(f"{path}: tensor {name} has non-finite values")
        tensors[name] = t.astype(np.float32)
        pos += 4 * count
    if pos != len(data):
        raise TrainerError(f"{path}: {len(data) - pos} trailing bytes after the payload")
    return SwmModel(header, tensors)
