"""Inning half — four-state DU matching ESPN's shortDetail prefix.

This module follows the HUB75 DU pattern: each variant is a plain class
in the module, and the union is expressed inline as
``Top | Middle | Bottom | End`` at the use site.
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
