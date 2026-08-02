"""Inning half — four-state DU matching ESPN's shortDetail prefix.

Each variant is a plain class with a single module-level instance. The
variants carry no data, so deserialization reuses the singletons (no
per-parse allocation) and consumers compare with identity:

    if half is TOP: ...

The union is expressed inline as ``Top | Middle | Bottom | End`` at the
use site, matching the HUB75 library's DU convention.
"""


class Top:
    """Top of the inning (away team batting)."""
    pass


class Middle:
    """Between top and bottom halves."""
    pass


class Bottom:
    """Bottom of the inning (home team batting)."""
    pass


class End:
    """Between bottom of this inning and top of the next."""
    pass


# Singleton instances — the only values that should ever circulate.
TOP = Top()
MIDDLE = Middle()
BOTTOM = Bottom()
END = End()
