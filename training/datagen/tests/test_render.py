from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from helpers import make_sofa
from stepwave_datagen import CLASSES, SR
from stepwave_datagen.config import load_config
from stepwave_datagen.hrtf import load_sofa
from stepwave_datagen.render import (
    active_rms,
    distance_gain,
    lpf_cutoff_hz,
    render_stems,
    synthetic_rir,
)
from stepwave_datagen.scene import Event, Scene

CFG = load_config(Path(__file__).resolve().parents[1] / "configs/cs2.toml")


@pytest.fixture
def hrtf(tmp_path: Path):
    path = tmp_path / "h.sofa"
    make_sofa(
        path, [(0, 0), (90, 0), (180, 0), (270, 0)], gains=[(1, 1), (1, 0.2), (1, 1), (0.2, 1)]
    )
    return load_sofa(path)


def noise(seconds: float, seed: int = 0) -> np.ndarray:
    return np.random.default_rng(seed).uniform(-0.5, 0.5, int(seconds * SR)).astype(np.float32)


SOURCES = {
    "step": noise(0.15, 1),
    "amb": noise(1.0, 2),
    "gun": noise(0.2, 3),
    "long": noise(30.0, 4),
}


def ev(
    source: str,
    cls: str,
    onset: float = 0.1,
    az: float = 0.0,
    dist: float = 2.0,
    spatial: bool = True,
    wet: float = 0.0,
    offset: float = 0.0,
) -> Event:
    return Event(source, cls, "", "", onset, az, 0.0, dist, spatial, wet, offset)


def scn(events: list[Event], snr: float | None = None, seconds: float = 2.0) -> Scene:
    return Scene("t-000000", 1, seconds, tuple(events), 0.4, 7, snr, -25.0)


def test_distance_helpers() -> None:
    assert distance_gain(0.1) == 2.0 and distance_gain(4.0) == 0.25
    assert lpf_cutoff_hz(1.0, CFG.render) == 20_000.0
    assert lpf_cutoff_hz(400.0, CFG.render) == 2_000.0


def test_rir_is_normalised_stereo() -> None:
    rir = synthetic_rir(0.5, 3)
    assert rir.shape == (2, int(0.5 * SR)) and rir.dtype == np.float32
    np.testing.assert_allclose((rir.astype(np.float64) ** 2).sum(axis=1), 1.0, rtol=1e-4)
    assert not np.allclose(rir[0], rir[1])


def test_stems_shape_and_classes(hrtf) -> None:
    stems = render_stems(
        scn([ev("amb", "ambience", onset=0.0)]), SOURCES.__getitem__, hrtf, CFG.render
    )
    assert tuple(stems) == CLASSES
    assert all(s.shape == (2, 2 * SR) and s.dtype == np.float32 for s in stems.values())
    assert np.abs(stems["ambience"]).max() > 0
    assert np.abs(stems["footsteps"]).max() == 0


def test_left_source_is_louder_left(hrtf) -> None:
    stems = render_stems(
        scn([ev("step", "footsteps", az=90.0)]), SOURCES.__getitem__, hrtf, CFG.render
    )
    left, right = stems["footsteps"]
    assert 20 * np.log10(np.sqrt(np.mean(left**2)) / np.sqrt(np.mean(right**2))) > 3.0


def test_non_spatial_is_centred(hrtf) -> None:
    stems = render_stems(
        scn([ev("gun", "gunfire", spatial=False, dist=0.5)]), SOURCES.__getitem__, hrtf, CFG.render
    )
    np.testing.assert_array_equal(stems["gunfire"][0], stems["gunfire"][1])


def test_ambience_loops_to_fill_clip(hrtf) -> None:
    stems = render_stems(
        scn([ev("amb", "ambience", onset=0.0, offset=0.7)], seconds=3.0),
        SOURCES.__getitem__,
        hrtf,
        CFG.render,
    )
    tail = stems["ambience"][:, -SR // 10 :]
    assert np.abs(tail).max() > 0


def test_long_event_is_truncated_to_clip(hrtf) -> None:
    stems = render_stems(
        scn([ev("long", "other", onset=1.5)]), SOURCES.__getitem__, hrtf, CFG.render
    )
    assert stems["other"].shape == (2, 2 * SR)
    assert np.abs(stems["other"][:, : int(1.4 * SR)]).max() == 0


def test_sources_are_truncated_before_filtering(hrtf, monkeypatch) -> None:
    import stepwave_datagen.render as render

    lengths: list[int] = []
    real_lowpass, real_conv = render._lowpass, render.fftconvolve

    def spy_lowpass(x, cutoff, cfg):
        lengths.append(len(x))
        return real_lowpass(x, cutoff, cfg)

    def spy_conv(a, b, *args, **kwargs):
        if a.ndim == 1 and len(b) <= 64:  # the HRTF convolution, not the reverb send
            lengths.append(len(a))
        return real_conv(a, b, *args, **kwargs)

    monkeypatch.setattr(render, "_lowpass", spy_lowpass)
    monkeypatch.setattr(render, "fftconvolve", spy_conv)
    onset = 1.5
    render_stems(
        scn([ev("long", "other", onset=onset, dist=30.0), ev("long", "gunfire", spatial=False)]),
        SOURCES.__getitem__,
        hrtf,
        CFG.render,
    )
    assert lengths and max(lengths) <= 2 * SR - int(0.1 * SR)
    assert int(round(onset * SR)) + min(lengths) <= 2 * SR


def test_truncation_matches_rendering_a_pre_cut_source(hrtf) -> None:
    onset = 1.5
    cut = {"cut": SOURCES["long"][: 2 * SR - int(round(onset * SR))]}
    events = [ev("long", "other", onset=onset, dist=30.0, wet=0.2)]
    full = render_stems(scn(events), SOURCES.__getitem__, hrtf, CFG.render)
    events = [ev("cut", "other", onset=onset, dist=30.0, wet=0.2)]
    pre = render_stems(scn(events), cut.__getitem__, hrtf, CFG.render)
    np.testing.assert_allclose(full["other"], pre["other"], atol=1e-6)


def test_footstep_snr_matches_target(hrtf) -> None:
    events = [ev("amb", "ambience", onset=0.0), ev("step", "footsteps", onset=0.5, wet=0.2)]
    for snr in (-25.0, -10.0, 0.0):
        stems = render_stems(scn(events, snr=snr), SOURCES.__getitem__, hrtf, CFG.render)
        rest = sum(stems[c] for c in CLASSES if c != "footsteps")
        measured = 20 * np.log10(active_rms(stems["footsteps"]) / active_rms(rest))
        assert abs(measured - snr) < 0.5


def test_render_is_deterministic(hrtf) -> None:
    s = scn([ev("amb", "ambience", onset=0.0, wet=0.3), ev("step", "footsteps", wet=0.2)], snr=-5.0)
    a = render_stems(s, SOURCES.__getitem__, hrtf, CFG.render)
    b = render_stems(s, SOURCES.__getitem__, hrtf, CFG.render)
    assert all(np.array_equal(a[c], b[c]) for c in CLASSES)
