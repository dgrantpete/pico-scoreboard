"""
Auto-brightness: pure functions mapping ambient lux to display brightness.

Uses log-scale mapping (matches human perception), EMA smoothing,
gradual ramping, and a dual-lerp user preference curve.
"""

import math

# Tunable constants
LUX_MIN = 2.0       # Lux at/below which display is at minimum brightness
LUX_MAX = 300.0     # Lux at/above which display is at maximum brightness
BRI_MIN = 0.05      # Minimum display brightness (never fully black)
BRI_MAX = 1.0       # Maximum display brightness
EMA_ALPHA = 0.08    # Lux smoothing (lower = slower response, less flicker)

# The auto-brightness loop ticks at this period (main.py sleeps on it), and
# RAMP_STEP is derived so the ramp speed is expressed in real units instead
# of silently changing whenever the tick rate does.
TICK_MS = 200                                # 5 Hz update rate
RAMP_PER_SECOND = 0.2                        # Max brightness change per second
RAMP_STEP = RAMP_PER_SECOND * TICK_MS / 1000  # Per-tick step (0.04 at 5 Hz)

_LOG_RANGE = math.log(LUX_MAX / LUX_MIN)


def smooth_lux(current: float, reading: float) -> float:
    """Exponential moving average filter for lux readings."""
    return current + EMA_ALPHA * (reading - current)


def lux_to_ambient(lux: float) -> float:
    """Log-scale map from lux to ambient brightness (BRI_MIN to BRI_MAX)."""
    t = math.log(max(lux, LUX_MIN) / LUX_MIN) / _LOG_RANGE
    if t < 0.0:
        t = 0.0
    elif t > 1.0:
        t = 1.0
    return BRI_MIN + t * (BRI_MAX - BRI_MIN)


def ramp(current: float, target: float) -> float:
    """Rate-limit brightness change per step."""
    delta = target - current
    if delta > RAMP_STEP:
        return current + RAMP_STEP
    if delta < -RAMP_STEP:
        return current - RAMP_STEP
    return target


def apply_preference(ambient: float, user_pref: int) -> float:
    """
    Dual-lerp combining ambient brightness with user preference.

    user_pref=0   → BRI_MIN (minimum brightness)
    user_pref=50  → pure auto (ambient controls everything)
    user_pref=100 → BRI_MAX (maximum brightness)
    """
    if user_pref <= 50:
        blend = user_pref / 50
        return BRI_MIN + blend * (ambient - BRI_MIN)
    blend = (user_pref - 50) / 50
    return ambient + blend * (BRI_MAX - ambient)
