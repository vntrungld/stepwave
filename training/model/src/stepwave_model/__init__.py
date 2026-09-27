"""stepwave footstep model: feature prep, training, .swm export and offline evaluation."""

SIGNALS = ("mix", "footsteps", "gunfire", "explosions", "ambience", "other")
STEMS = SIGNALS[1:]
NUM_BANDS = 32
HOP = 480
SAMPLE_RATE = 48_000
