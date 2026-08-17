"""The threshold sweep must be able to express an improvement.

Run 4 swept 19 fixed points across [0.05, 0.95] and reported the same recall
at all of them, so Low, Medium and High collapsed onto two thresholds and no
change to the head could have moved the report. Candidate thresholds are now
drawn from the positive scores themselves, and each named point is the most
sensitive threshold inside its own false-accept budget.
"""

from __future__ import annotations

import numpy as np

from wake_word_training.evaluate_lib import (
    _candidate_thresholds,
    _operating_points,
    _threshold_curve,
)
from wake_word_training.gates import FALSE_ACCEPT_BUDGETS, OPERATING_POINT_NAMES


BACKGROUND_HOURS = 50.0
BACKGROUND_WINDOWS = 20_000
# 300 of the background windows reach into the positives' range, so 50 h of
# it costs 12 false accepts an hour just above 0.5 and the three budgets land
# on three different thresholds.
BACKGROUND_TAIL = 300


def _positives(seed: int, n: int = 500) -> list[float]:
    return list(np.random.default_rng(seed).uniform(0.5, 1.0, n))


def _background(seed: int) -> list[float]:
    rng = np.random.default_rng(seed)
    scores = np.concatenate(
        [
            rng.uniform(0.0, 0.5, BACKGROUND_WINDOWS - BACKGROUND_TAIL),
            rng.uniform(0.5, 1.0, BACKGROUND_TAIL),
        ]
    )
    # Shuffled, because the refractory window suppresses runs of hits and a
    # contiguous block of loud windows would flatter the count.
    rng.shuffle(scores)
    return list(scores)


def _points(seed: int) -> dict:
    curve = _threshold_curve(_positives(seed), _background(seed + 1), BACKGROUND_HOURS)
    return {p.name: p for p in _operating_points(curve)}


def test_candidates_reach_where_the_positives_actually_are() -> None:
    # A head calibrated into a narrow band gets two usable grid points out of
    # 19; the quantile candidates put a threshold between every pair of clips.
    positives = list(np.random.default_rng(1).uniform(0.90, 0.999, 500))
    inside = [t for t in _candidate_thresholds(positives) if 0.90 <= t <= 0.999]
    assert len(inside) > 19, f"only {len(inside)} candidates inside the score band"


def test_candidates_reach_the_thin_top_of_the_background() -> None:
    # The false-accept budgets are decided by the few hundred loudest-scoring
    # background windows out of a million. Without candidates up there, Low and
    # Medium land on the same threshold despite budgets five times apart.
    rng = np.random.default_rng(11)
    background = list(
        np.concatenate([rng.uniform(0.0, 0.05, 20_000), rng.uniform(0.9, 0.99, 200)])
    )
    positives = list(rng.uniform(0.995, 0.999, 500))
    candidates = _candidate_thresholds(positives, background)
    between = [t for t in candidates if 0.95 < t < 0.995]
    assert len(between) >= 5, f"only {len(between)} candidates in the gap"


def test_recall_moves_across_the_sweep() -> None:
    curve = _threshold_curve(_positives(2), _background(3), BACKGROUND_HOURS)
    recalls = {round(recall, 3) for _t, _fa, recall in curve}
    assert len(recalls) > 20, f"recall took only {len(recalls)} values"
    assert max(recalls) > 0.9 and min(recalls) < 0.1


def test_each_named_point_stays_inside_its_false_accept_budget() -> None:
    points = _points(4)
    assert set(points) == set(OPERATING_POINT_NAMES)
    for name, point in points.items():
        assert point.false_accepts_per_hour <= FALSE_ACCEPT_BUDGETS[name] + 1e-9


def test_the_three_points_trade_recall_against_false_accepts() -> None:
    points = _points(6)
    assert points["Low"].threshold > points["Medium"].threshold > points["High"].threshold
    assert points["Low"].recall < points["Medium"].recall < points["High"].recall


def test_three_distinct_points_survive_a_thin_background_tail() -> None:
    # The shipped head's shape: positives packed into a narrow band with the
    # background's tail reaching just under it, and nothing between the top of
    # the fixed grid and that band unless the background supplies it.
    rng = np.random.default_rng(21)
    background = list(
        np.concatenate([rng.uniform(0.0, 0.05, 20_000), rng.uniform(0.95, 0.9834, 200)])
    )
    rng.shuffle(background)
    positives = list(rng.uniform(0.9835, 0.986, 500))
    points = {
        p.name: p
        for p in _operating_points(
            _threshold_curve(positives, background, BACKGROUND_HOURS)
        )
    }
    assert len({p.threshold for p in points.values()}) == 3


def test_a_pinned_head_is_reported_as_one_point_not_three() -> None:
    # The failure this protocol has to keep visible: scores on the rails leave
    # nothing to trade, and the report must not dress that up as a curve.
    positives = [1.0] * 300 + [0.0] * 300
    background = [0.0] * 19_000 + [1.0] * 1_000
    points = {
        p.name: p
        for p in _operating_points(
            _threshold_curve(positives, background, BACKGROUND_HOURS)
        )
    }
    assert len({p.recall for p in points.values()}) == 1


def test_an_unaffordable_curve_still_reports_a_measured_number() -> None:
    # Background pinned at the top: no threshold can buy its way under any
    # budget, and the report must say so with a measured number.
    positives = list(np.random.default_rng(8).uniform(0.9, 1.0, 100))
    background = [1.0] * BACKGROUND_WINDOWS
    points = {
        p.name: p
        for p in _operating_points(
            _threshold_curve(positives, background, BACKGROUND_HOURS)
        )
    }
    assert points["Medium"].false_accepts_per_hour > FALSE_ACCEPT_BUDGETS["Medium"]
    assert np.isfinite(points["Medium"].false_accepts_per_hour)
