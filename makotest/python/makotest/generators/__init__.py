"""Generators for regulator-shaped test data.

Pure Python — no regulatory rule table lives here, so `numpy`-flavoured
convenience beats a Rust binding.
"""

from .epex import EpexSim, MtuPrice, Profile

__all__ = ["EpexSim", "MtuPrice", "Profile"]
