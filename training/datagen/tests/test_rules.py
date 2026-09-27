from __future__ import annotations

from pathlib import Path

import pytest

from stepwave_datagen.errors import DatagenError
from stepwave_datagen.rules import Label, classify, load_rules

RULES = """
[[rule]]
pattern = "sounds/player/footsteps/{surface}/**"
class = "footsteps"

[[rule]]
pattern = "sounds/weapons/hegrenade/*.wav"
class = "explosions"

[[rule]]
pattern = "sounds/weapons/**"
class = "gunfire"

[[rule]]
pattern = "sounds/ambient/{map}/**"
class = "ambience"

[[rule]]
pattern = "sounds/player/land*.wav"
class = "footsteps"
surface = "generic"
"""


@pytest.fixture
def rules(tmp_path: Path):
    path = tmp_path / "rules.toml"
    path.write_text(RULES)
    return load_rules(path)


def test_capture_surface(rules) -> None:
    assert classify("sounds/player/footsteps/wood/step_01.wav", rules) == Label(
        "footsteps", "wood", ""
    )


def test_capture_map_at_depth(rules) -> None:
    assert classify("sounds/ambient/dust2/birds/loop.wav", rules) == Label("ambience", "", "dust2")


def test_first_match_wins(rules) -> None:
    assert classify("sounds/weapons/hegrenade/explode3.wav", rules).cls == "explosions"
    assert classify("sounds/weapons/hegrenade/sub/pin.wav", rules).cls == "gunfire"


def test_literal_surface_and_single_segment_star(rules) -> None:
    assert classify("sounds/player/land_01.wav", rules) == Label("footsteps", "generic", "")
    assert classify("sounds/player/sub/land_01.wav", rules).cls == "other"


def test_unmatched_is_other(rules) -> None:
    assert classify("sounds/ui/click.wav", rules) == Label("other", "", "")


def test_unknown_class_rejected(tmp_path: Path) -> None:
    path = tmp_path / "bad.toml"
    path.write_text('[[rule]]\npattern = "a/**"\nclass = "music"\n')
    with pytest.raises(DatagenError, match="music"):
        load_rules(path)
