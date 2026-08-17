"""Confidence intervals for the two gated measurements.

Both gates are counts, not continuous quantities: 17 false accepts over 46 h
and 1986 hits over 2000 clips. A point estimate printed to four significant
figures hides that 17 events place the true rate anywhere between 0.215 and
0.590 per hour, which straddles the 0.5 ceiling. The gate judges the
conservative end of each interval, so both ends have to be computed here.

Pure stdlib on purpose: `scipy` lives in the `train` extra, and these
functions are imported by the gate check, which must run on a bare install.
"""

from __future__ import annotations

import math

# Two-sided 95%, and only that: a gate with a tunable confidence is a gate with
# a dial on it.
CONFIDENCE = 0.95
_TAIL = (1.0 - CONFIDENCE) / 2.0
# Standard normal quantile at 1 - _TAIL, for the Wilson interval.
_Z = 1.959963984540054


def poisson_rate_interval(events: int, exposure: float) -> tuple[float, float]:
    """Exact (Garwood) interval for a rate of `events` over `exposure` units.

    The bounds invert the Poisson CDF rather than approximating it, because a
    normal or Wald approximation is wrong at the counts this gate produces:
    at 17 events the Wald interval is 0.20 to 0.54 while the exact one runs
    0.21 to 0.59, and the ceiling sits inside the difference.
    """
    if events < 0:
        raise ValueError("events must be non-negative")
    if exposure <= 0:
        return (0.0, math.inf)
    lower = 0.0 if events == 0 else _solve_mean(events - 1, 1.0 - _TAIL)
    upper = _solve_mean(events, _TAIL)
    return (lower / exposure, upper / exposure)


def wilson_proportion_interval(hits: int, trials: int) -> tuple[float, float]:
    """Wilson score interval for `hits` out of `trials`.

    Wilson rather than Wald because recall sits near 1.0, where Wald puts the
    upper bound above 1 and understates the width.
    """
    if trials <= 0:
        return (0.0, 1.0)
    if not 0 <= hits <= trials:
        raise ValueError("hits must lie in [0, trials]")
    z = _Z
    p = hits / trials
    denom = 1.0 + z * z / trials
    centre = (p + z * z / (2 * trials)) / denom
    half = z / denom * math.sqrt(p * (1.0 - p) / trials + z * z / (4 * trials * trials))
    return (max(0.0, centre - half), min(1.0, centre + half))


def _poisson_cdf(k: int, mean: float) -> float:
    """P(X <= k) for X ~ Poisson(mean), summed with a stable recurrence."""
    if mean <= 0.0:
        return 1.0
    term = math.exp(-mean)
    total = term
    for i in range(1, k + 1):
        term *= mean / i
        total += term
    return min(total, 1.0)


def _solve_mean(k: int, target_cdf: float) -> float:
    """The Poisson mean whose CDF at `k` equals `target_cdf`.

    The CDF decreases monotonically in the mean, so bisection is exact to
    within the tolerance and needs no special functions.
    """
    low = 0.0
    high = max(1.0, k + 10.0 * math.sqrt(k + 1.0))
    while _poisson_cdf(k, high) > target_cdf:
        high *= 2.0
        if high > 1e6:
            return high
    for _ in range(200):
        mid = 0.5 * (low + high)
        if _poisson_cdf(k, mid) > target_cdf:
            low = mid
        else:
            high = mid
        if high - low < 1e-12 * max(1.0, high):
            break
    return 0.5 * (low + high)
