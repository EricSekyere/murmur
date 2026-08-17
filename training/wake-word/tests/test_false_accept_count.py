"""The arithmetic behind the most load-bearing number in the release.

Nothing in the suite pinned it. Changing `hits / hours` to `hits / hours / 2.0`
passed all 91 tests, because every other test asserts a relation (this budget
holds, that point is looser than the other) and a constant factor preserves
every relation. These tests are absolute: hand-counted events over a
hand-chosen exposure, equal to a hand-computed rate.

They also pin the de-duplication, which is where the eval and the shipped
detector had genuinely drifted apart.
"""

from __future__ import annotations

import numpy as np

from wake_word_training.calibration import (
    REFRACTORY_WINDOWS,
    SERVE_REFRACTORY_FRAMES,
    SERVE_REFRACTORY_SECONDS,
    false_accept_events,
    false_accepts_per_hour,
)
from wake_word_training.features import FRAME_SAMPLES, WINDOW_STRIDE_SECONDS
from wake_word_training.audio import SAMPLE_RATE


def _isolated_hits(count: int, spacing: int = 40) -> list[float]:
    """`count` hits far enough apart that no refractory can merge them."""
    scores = [0.0] * (count * spacing)
    for index in range(count):
        scores[index * spacing] = 0.9
    return scores


def test_the_rate_is_events_divided_by_hours_and_nothing_else() -> None:
    # Three hits over exactly two hours is 1.5 per hour. A stray factor
    # anywhere in the division shows up here and nowhere else.
    three = _isolated_hits(3)
    assert false_accept_events([three], 0.5) == 3
    assert false_accepts_per_hour([three], 0.5, 2.0) == 1.5
    assert false_accepts_per_hour([three], 0.5, 0.5) == 6.0
    assert false_accepts_per_hour([three], 0.95, 2.0) == 0.0


def test_a_hand_counted_corpus_gives_a_hand_computed_rate() -> None:
    # File A: 20 consecutive windows above the threshold. The first fires and
    # the next REFRACTORY_WINDOWS (15) are suppressed, so window 16 fires too:
    # 2 events, not 20 and not 1.
    # File B: hits at windows 0 and 30, 30 > 15 apart: 2 more events.
    # 4 events over half an hour is exactly 8 per hour.
    run_of_twenty = [0.9] * 20
    two_apart = [0.0] * 40
    two_apart[0] = 0.9
    two_apart[30] = 0.9
    corpus = [run_of_twenty, two_apart]
    assert false_accept_events(corpus, 0.6) == 4
    assert false_accepts_per_hour(corpus, 0.6, 0.5) == 8.0


def test_the_refractory_collapses_a_run_into_one_event_per_window_block() -> None:
    # Two full blocks of (1 firing window + REFRACTORY_WINDOWS suppressed).
    scores = [0.9] * (2 * (REFRACTORY_WINDOWS + 1))
    assert false_accept_events([scores], 0.5) == 2
    # One window short of the second block, and only the first fires.
    assert false_accept_events([scores[:-1]], 0.5) == 2
    assert false_accept_events([[0.9] * (REFRACTORY_WINDOWS + 1)], 0.5) == 1


def test_the_refractory_ends_exactly_where_the_serve_one_does() -> None:
    def events_with_gap(gap: int) -> int:
        scores = [0.0] * (gap + 2)
        scores[0] = 0.9
        scores[gap] = 0.9
        return false_accept_events([scores], 0.5)

    assert events_with_gap(REFRACTORY_WINDOWS) == 1
    assert events_with_gap(REFRACTORY_WINDOWS + 1) == 2


def test_the_eval_refractory_covers_the_serve_refractory_and_no_more() -> None:
    # murmur-core's WAKE_REFRACTORY_FRAMES is 25 frames of 80 ms = 2.0 s. The
    # eval slides one head window per 128 ms, so counting 25 windows suppressed
    # 3.2 s where the shipped detector suppresses 2.0 s, and the gate
    # undercounted its own false accepts by a third.
    assert SERVE_REFRACTORY_FRAMES * FRAME_SAMPLES / SAMPLE_RATE == SERVE_REFRACTORY_SECONDS
    assert SERVE_REFRACTORY_SECONDS == 2.0
    assert WINDOW_STRIDE_SECONDS == 0.128
    assert REFRACTORY_WINDOWS == 15
    # Rounded down, never up: the eval must not suppress longer than serve.
    assert REFRACTORY_WINDOWS * WINDOW_STRIDE_SECONDS <= SERVE_REFRACTORY_SECONDS
    assert (REFRACTORY_WINDOWS + 1) * WINDOW_STRIDE_SECONDS > SERVE_REFRACTORY_SECONDS


def test_the_refractory_does_not_carry_across_a_file_boundary() -> None:
    # Two unrelated recordings that each fire on their first window are two
    # false accepts. Scoring them as one concatenated stream reported one,
    # which is the shape of the bleed the flat score list used to have.
    one_file = [0.9] + [0.0] * 5
    assert false_accept_events([one_file, list(one_file)], 0.5) == 2
    assert false_accept_events([one_file + list(one_file)], 0.5) == 1


def test_a_score_exactly_at_the_threshold_is_a_hit() -> None:
    # The detector fires on `score >= threshold`, and the threshold candidates
    # are built by nudging around that boundary, so the comparison matters.
    assert false_accept_events([[0.5]], 0.5) == 1
    assert false_accept_events([[np.nextafter(0.5, 0.0)]], 0.5) == 0


def test_zero_exposure_is_infinite_rather_than_a_division_error() -> None:
    assert false_accepts_per_hour([[0.9]], 0.5, 0.0) == float("inf")
    assert false_accept_events([], 0.5) == 0
