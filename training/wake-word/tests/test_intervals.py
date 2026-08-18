"""The intervals the gates are judged on, pinned against known values."""

from __future__ import annotations

import math

import pytest

from wake_word_training.intervals import (
    poisson_rate_interval,
    wilson_proportion_interval,
)


def test_the_shipped_measurement_reproduces_its_published_interval() -> None:
    # 17 false accepts over 46.1446 h read as 0.3684/hour. The exact Poisson
    # interval is 0.215 to 0.590, which straddles the 0.5 ceiling, and that is
    # the whole reason the gate judges the upper end.
    low, high = poisson_rate_interval(17, 46.144601597222255)
    assert low == pytest.approx(0.2146, abs=5e-4)
    assert high == pytest.approx(0.5899, abs=5e-4)


def test_the_bounds_match_the_chi_square_form() -> None:
    # Garwood: lower = chi2(0.025, 2k)/2, upper = chi2(0.975, 2k+2)/2.
    scipy_stats = pytest.importorskip("scipy.stats")
    for events in (0, 1, 5, 17, 40):
        low, high = poisson_rate_interval(events, 1.0)
        expected_high = scipy_stats.chi2.ppf(0.975, 2 * events + 2) / 2.0
        assert high == pytest.approx(expected_high, rel=1e-6)
        if events:
            expected_low = scipy_stats.chi2.ppf(0.025, 2 * events) / 2.0
            assert low == pytest.approx(expected_low, rel=1e-6)


def test_zero_events_has_a_zero_lower_bound_and_a_finite_upper_one() -> None:
    low, high = poisson_rate_interval(0, 7.38)
    assert low == 0.0
    # 3 / exposure is the usual rule of thumb for the 95% upper bound at zero
    # events, and this is the exact version of it.
    assert high == pytest.approx(3.689 / 7.38, rel=1e-3)


def test_the_interval_narrows_as_exposure_grows() -> None:
    widths = [
        poisson_rate_interval(round(0.37 * hours), hours)[1]
        - poisson_rate_interval(round(0.37 * hours), hours)[0]
        for hours in (46.0, 200.0, 1000.0)
    ]
    assert widths[0] > widths[1] > widths[2]


def test_zero_exposure_cannot_bound_a_rate() -> None:
    assert poisson_rate_interval(3, 0.0) == (0.0, math.inf)


def test_recall_on_two_thousand_clips_is_tight() -> None:
    # 1986/2000 is 0.9930, and the interval is 0.9883 to 0.9958: two orders of
    # magnitude clear of the 0.9 floor, so which end is judged changes nothing.
    low, high = wilson_proportion_interval(1986, 2000)
    assert low == pytest.approx(0.9883, abs=5e-4)
    assert high == pytest.approx(0.9958, abs=5e-4)
    assert low > 0.9


def test_wilson_stays_inside_zero_and_one_at_the_edges() -> None:
    assert wilson_proportion_interval(10, 10)[1] <= 1.0
    assert wilson_proportion_interval(0, 10)[0] == 0.0
    # A perfect score on ten clips is not evidence of a 0.9 recall floor.
    assert wilson_proportion_interval(10, 10)[0] < 0.9
