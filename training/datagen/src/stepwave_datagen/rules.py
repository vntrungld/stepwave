"""Path-pattern rules that assign each extracted sound a class, surface and map."""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

from . import CLASSES
from .errors import DatagenError

_TOKENS = re.compile(r"(\*\*/|\*\*|\*|\?|\{surface\}|\{map\})")


@dataclass(frozen=True)
class Rule:
    pattern: str
    cls: str
    surface: str | None
    map: str | None
    regex: re.Pattern[str]


@dataclass(frozen=True)
class Label:
    cls: str
    surface: str
    map: str


def _compile(pattern: str) -> re.Pattern[str]:
    parts = []
    for token in _TOKENS.split(pattern):
        if token == "**/":
            parts.append(r"(?:.*/)?")
        elif token == "**":
            parts.append(r".*")
        elif token == "*":
            parts.append(r"[^/]*")
        elif token == "?":
            parts.append(r"[^/]")
        elif token == "{surface}":
            parts.append(r"(?P<surface>[^/]+)")
        elif token == "{map}":
            parts.append(r"(?P<map>[^/]+)")
        else:
            parts.append(re.escape(token))
    return re.compile("".join(parts) + r"\Z")


def load_rules(path: Path) -> list[Rule]:
    try:
        data = tomllib.loads(path.read_text())
    except FileNotFoundError as err:
        raise DatagenError(f"rules file not found: {path}") from err
    except tomllib.TOMLDecodeError as err:
        raise DatagenError(f"{path}: invalid TOML ({err}); fix the syntax and re-run") from err
    rules = []
    for i, entry in enumerate(data.get("rule", [])):
        cls = entry.get("class")
        if cls not in CLASSES:
            raise DatagenError(f"{path}: rule {i} has class {cls!r}; expected one of {CLASSES}")
        pattern = entry.get("pattern")
        if not pattern:
            raise DatagenError(f"{path}: rule {i} has no 'pattern'")
        rules.append(Rule(pattern, cls, entry.get("surface"), entry.get("map"), _compile(pattern)))
    return rules


def classify(rel_path: str, rules: list[Rule]) -> Label:
    for rule in rules:
        m = rule.regex.match(rel_path)
        if m:
            groups = m.groupdict()
            surface = groups.get("surface") or rule.surface or ""
            map_name = groups.get("map") or rule.map or ""
            return Label(rule.cls, surface, map_name)
    return Label("other", "", "")
