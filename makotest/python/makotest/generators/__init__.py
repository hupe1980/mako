"""Generators for regulator-shaped test data.

Pure Python — no regulatory rule table lives here, so `numpy`-flavoured
convenience beats a Rust binding.
"""

from .epex import EpexGenerator, MtuPrice, Profile

__all__ = ["EpexGenerator", "MtuPrice", "Profile"]
