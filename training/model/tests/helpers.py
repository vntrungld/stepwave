"""Shared test fixtures (synthetic only, no game assets)."""

from __future__ import annotations

from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
CONFIG_PATH = REPO / "training/model/configs/cs2.toml"
PROFILE = REPO / "profiles/cs2.json"
