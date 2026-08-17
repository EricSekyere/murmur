"""Choosing thresholds on validation data, then measuring them once elsewhere.

The old protocol built candidate thresholds from the held-out positives and the
gate background, picked the loosest threshold whose false-accept rate on those
same background scores fitted the budget, and then reported that rate and that
recall. Selection and measurement shared one sample, so the reported numbers
were the best of ~126 tries rather than an estimate of anything.

`calibrate` therefore sees only the validation half (validation speakers and
the validation background split), and `measure` sees only the held-out half.
Nothing in `measure` can move a threshold.
"""

from __future__ import annotations

from typing import NamedTuple, Sequence

from wake_word_training.audio import SAMPLE_RATE
from wake_word_training.features import FRAME_SAMPLES, WINDOW_STRIDE_SECONDS
from wake_word_training.gates import (
    FALSE_ACCEPT_BUDGETS,
    OPERATING_POINT_NAMES,
    OperatingPoint,
)
from wake_word_training.intervals import (
    poisson_rate_interval,
    wilson_proportion_interval,
)

# Thresholds are drawn from the positive scores themselves as well as a fixed
# grid. A head whose scores all sat at 0.0000 or 1.0000 had nothing to trade
# on a [0.05, 0.95] grid, and the sweep reported the same recall at all 19
# points; quantile thresholds put a candidate wherever positives actually are.
SCORE_QUANTILES = 41
# The background's upper tail decides the false-accept budgets, and it is
# thin: 99% of windows sit below 0.04 while the handful that matter sit near
# 1.0. Sampling it logarithmically puts candidates between the top of the
# fixed grid and the positive band, where Low and Medium otherwise collapse
# onto one threshold despite budgets that differ fivefold.
TAIL_QUANTILES = 25
TAIL_DECADES = 5

# The armed loop ignores WAKE_REFRACTORY_FRAMES frames of 80 ms after a hit
# (murmur-core audio/wake.rs), so one utterance cannot double-trigger.
SERVE_REFRACTORY_FRAMES = 25
SERVE_REFRACTORY_SECONDS = SERVE_REFRACTORY_FRAMES * FRAME_SAMPLES / SAMPLE_RATE
# The same suppression measured in eval windows, which slide every 128 ms and
# not every 80 ms. Counting 25 windows suppressed 3.2 s where the shipped
# detector suppresses 2.0 s, so the gate undercounted its own false accepts.
# Rounded down: the eval window must never outlast the serve window, or the
# count is flattered by the arithmetic rather than by the head.
REFRACTORY_WINDOWS = int(SERVE_REFRACTORY_SECONDS / WINDOW_STRIDE_SECONDS)


class CurvePoint(NamedTuple):
    threshold: float
    false_accepts_per_hour: float
    recall: float


class Chosen(NamedTuple):
    """A threshold and what it did on the data that chose it."""

    name: str
    threshold: float
    false_accepts_per_hour: float
    recall: float
    events: int
    hours: float
    clips: int


def false_accept_events(segments: Sequence[Sequence[float]], threshold: float) -> int:
    """Hits above `threshold`, de-duplicated by the serve refractory.

    `segments` holds one score list per file. The refractory resets between
    them: a hit at the end of one recording has no business suppressing the
    start of an unrelated one, and letting it do so hid false accepts.
    """
    hits = 0
    for scores in segments:
        refractory = 0
        for score in scores:
            if refractory > 0:
                refractory -= 1
                continue
            if score >= threshold:
                hits += 1
                refractory = REFRACTORY_WINDOWS
    return hits


def false_accepts_per_hour(
    segments: Sequence[Sequence[float]], threshold: float, hours: float
) -> float:
    if hours <= 0:
        return float("inf")
    return false_accept_events(segments, threshold) / hours


def recall_at(pos_scores: Sequence[float], threshold: float) -> float:
    if not len(pos_scores):
        return 0.0
    return sum(1 for score in pos_scores if score >= threshold) / len(pos_scores)


def candidate_thresholds(pos_scores, bg_scores=()) -> list[float]:
    import numpy as np

    grid = list(np.linspace(0.05, 0.95, 19))
    positives = list(np.quantile(pos_scores, np.linspace(0.0, 1.0, SCORE_QUANTILES)))
    # A threshold exactly at a positive score keeps that clip (>=), so nudging
    # candidates down by an epsilon is what separates neighbouring recalls;
    # nudging a background candidate up is what excludes that window.
    candidates = grid + positives + [q - 1e-6 for q in positives]
    if len(bg_scores):
        tail = np.quantile(
            bg_scores, 1.0 - np.logspace(-TAIL_DECADES, -1, TAIL_QUANTILES)
        )
        candidates += [float(q) + 1e-6 for q in tail]
    return sorted(
        {round(float(np.clip(t, 1e-4, 1.0 - 1e-4)), 8) for t in candidates}
    )


def threshold_curve(
    pos_scores, bg_segments: Sequence[Sequence[float]], hours: float
) -> list[CurvePoint]:
    flat = [score for scores in bg_segments for score in scores]
    return [
        CurvePoint(
            threshold,
            false_accepts_per_hour(bg_segments, threshold, hours),
            recall_at(pos_scores, threshold),
        )
        for threshold in candidate_thresholds(pos_scores, flat)
    ]


def calibrate(
    pos_scores,
    bg_segments: Sequence[Sequence[float]],
    hours: float,
    *,
    curve: list[CurvePoint] | None = None,
) -> list[Chosen]:
    """The loosest threshold inside each budget, judged on validation only.

    The budget is compared against the validation point estimate rather than
    against an interval: 7.4 h of validation background resolves the rate only
    to 0.135/hour, so an interval-based rule here would accept nothing but a
    zero-event threshold and would buy that with recall the head does not have
    to give. Selection stays unbiased instead, and the interval is applied
    where the claim is made, on the held-out measurement.
    """
    sweep = curve if curve is not None else threshold_curve(pos_scores, bg_segments, hours)
    chosen: list[Chosen] = []
    for name in OPERATING_POINT_NAMES:
        budget = FALSE_ACCEPT_BUDGETS[name]
        affordable = [point for point in sweep if point.false_accepts_per_hour <= budget]
        # Nothing affordable means every threshold overspends even here; take
        # the cheapest so the gate fails on a real measured number.
        point = (
            min(affordable, key=lambda p: p.threshold)
            if affordable
            else min(sweep, key=lambda p: (p.false_accepts_per_hour, -p.threshold))
        )
        chosen.append(
            Chosen(
                name=name,
                threshold=point.threshold,
                false_accepts_per_hour=point.false_accepts_per_hour,
                recall=point.recall,
                events=false_accept_events(bg_segments, point.threshold),
                hours=hours,
                clips=len(pos_scores),
            )
        )
    return chosen


def measure(
    chosen: Chosen,
    pos_scores,
    bg_segments: Sequence[Sequence[float]],
    hours: float,
) -> OperatingPoint:
    """What a frozen threshold does on data that played no part in choosing it."""
    events = false_accept_events(bg_segments, chosen.threshold)
    rate = float("inf") if hours <= 0 else events / hours
    hits = sum(1 for score in pos_scores if score >= chosen.threshold)
    clips = len(pos_scores)
    return OperatingPoint(
        name=chosen.name,
        threshold=chosen.threshold,
        false_accepts_per_hour=rate,
        recall=hits / clips if clips else 0.0,
        false_accept_events=events,
        measured_hours=hours,
        measured_clips=clips,
        false_accepts_ci95=list(poisson_rate_interval(events, hours)),
        recall_ci95=list(wilson_proportion_interval(hits, clips)),
        calibration={
            "false_accepts_per_hour": chosen.false_accepts_per_hour,
            "false_accept_events": chosen.events,
            "recall": chosen.recall,
            "background_hours": chosen.hours,
            "clips": chosen.clips,
        },
    )
