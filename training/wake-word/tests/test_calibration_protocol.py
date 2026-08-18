"""Thresholds must be chosen on data that does not then report on them.

The old protocol built candidates from the held-out positives and the gate
background, picked the loosest threshold whose rate on those same background
scores fitted the budget, and reported that rate. Every gate number was the
best of ~126 tries on the sample it was measured against.
"""

from __future__ import annotations

import numpy as np

from wake_word_training.calibration import calibrate, measure
from wake_word_training.gates import FALSE_ACCEPT_BUDGETS


CALIBRATION_HOURS = 8.0
GATE_HOURS = 50.0


def _band(seed: int, low: float, high: float, n: int) -> list[float]:
    return list(np.random.default_rng(seed).uniform(low, high, n))


def _background(seed: int, *, tail: int, files: int = 20) -> list[list[float]]:
    rng = np.random.default_rng(seed)
    scores = np.concatenate(
        [rng.uniform(0.0, 0.05, 20_000 - tail), rng.uniform(0.90, 0.999, tail)]
    )
    rng.shuffle(scores)
    return [list(chunk) for chunk in np.array_split(scores, files)]


def test_the_chosen_threshold_does_not_depend_on_the_gate_data() -> None:
    # The structural property. Calibration is handed only the validation half,
    # so no gate sample, however different, can move a threshold.
    positives = _band(1, 0.95, 0.999, 400)
    chosen = calibrate(positives, _background(2, tail=200), CALIBRATION_HOURS)
    clean = [
        measure(one, _band(3, 0.95, 0.999, 2000), _background(4, tail=0), GATE_HOURS)
        for one in chosen
    ]
    noisy = [
        measure(one, _band(5, 0.0, 0.2, 2000), _background(6, tail=5_000), GATE_HOURS)
        for one in chosen
    ]
    assert [p.threshold for p in clean] == [p.threshold for p in noisy]
    # And the measurement does move, so the test is not passing vacuously.
    assert clean[1].false_accepts_per_hour < noisy[1].false_accepts_per_hour
    assert clean[1].recall > noisy[1].recall


def test_the_reported_numbers_come_from_the_gate_half() -> None:
    positives = _band(7, 0.95, 0.999, 400)
    chosen = calibrate(positives, _background(8, tail=300), CALIBRATION_HOURS)
    medium = next(one for one in chosen if one.name == "Medium")
    gate_background = _background(9, tail=4_000)
    point = measure(medium, _band(10, 0.9, 0.999, 1000), gate_background, GATE_HOURS)

    assert point.measured_hours == GATE_HOURS
    assert point.measured_clips == 1000
    assert point.false_accept_events > 0
    assert point.false_accepts_per_hour == point.false_accept_events / GATE_HOURS
    # The calibration numbers are kept beside them, not in place of them: a
    # threshold that only worked on the sample that chose it is now visible.
    assert point.calibration["background_hours"] == CALIBRATION_HOURS
    assert point.calibration["clips"] == 400
    assert point.calibration["false_accepts_per_hour"] <= FALSE_ACCEPT_BUDGETS["Medium"]


def test_the_measured_rate_carries_an_interval_around_it() -> None:
    positives = _band(11, 0.95, 0.999, 400)
    chosen = calibrate(positives, _background(12, tail=200), CALIBRATION_HOURS)
    point = measure(
        next(one for one in chosen if one.name == "Medium"),
        _band(13, 0.95, 0.999, 2000),
        _background(14, tail=40),
        GATE_HOURS,
    )
    low, high = point.false_accepts_ci95
    assert low <= point.false_accepts_per_hour <= high
    assert low < high, "an interval of zero width is not an interval"
    recall_low, recall_high = point.recall_ci95
    assert recall_low <= point.recall <= recall_high


def test_a_threshold_that_only_worked_on_its_own_sample_is_visible() -> None:
    # A calibration half whose tail happens to sit below the positives picks a
    # threshold that costs nothing there. If the gate half is worse behaved, the
    # report now shows both numbers instead of only the flattering one.
    positives = _band(15, 0.95, 0.999, 400)
    chosen = calibrate(positives, _background(16, tail=0), CALIBRATION_HOURS)
    medium = next(one for one in chosen if one.name == "Medium")
    point = measure(
        medium, _band(17, 0.95, 0.999, 2000), _background(18, tail=8_000), GATE_HOURS
    )
    assert point.calibration["false_accepts_per_hour"] <= FALSE_ACCEPT_BUDGETS["Medium"]
    assert point.false_accepts_per_hour > FALSE_ACCEPT_BUDGETS["Medium"]
    assert point.false_accepts_per_hour > 10 * point.calibration["false_accepts_per_hour"]
