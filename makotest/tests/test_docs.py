"""The published docs must not drift from the code they describe.

Prose drifts silently: a fixture is renamed, a strategy added, a subcommand
introduced, and the table listing them stays as it was. Nothing fails, and the
next reader trusts the table. The checks here are mechanical for exactly that
reason — a table that enumerates part of the API is pinned against the API.

Only tables with a machine-checkable counterpart are covered. A claim about
*why* the design is what it is cannot be checked here and is not attempted.

Skipped when the docs are absent, so an installed wheel's test run does not
fail on a repository layout it never had.
"""

from __future__ import annotations

import pathlib
import re

import pytest

from makotest import cli, plugin, strategies

_REPO = pathlib.Path(__file__).resolve().parents[2]
SITE = _REPO / "site/content/docs/reference/makotest.md"
README = _REPO / "makotest/README.md"

#: Names in `plugin.__all__` that are pytest hooks rather than fixtures.
_HOOKS = {"pytest_addoption", "pytest_configure"}


def _read(path: pathlib.Path) -> str:
    if not path.is_file():
        pytest.skip(f"{path.name} is not present in this layout")
    return path.read_text(encoding="utf-8")


def _fixture_row(text: str) -> set[str]:
    row = re.search(r"\*\*Fixtures\*\*\s*\|([^|]*)\|", text)
    assert row, "no Fixtures row in the doc's table"
    return set(re.findall(r"`([a-z_]+)`", row.group(1)))


@pytest.mark.parametrize("path", [SITE, README], ids=["site", "readme"])
def test_the_fixtures_table_lists_exactly_the_plugin_fixtures(path):
    """A fixture absent from the table is one nobody discovers."""
    listed = _fixture_row(_read(path))
    expected = set(plugin.__all__) - _HOOKS
    assert listed == expected


@pytest.mark.parametrize("path", [SITE, README], ids=["site", "readme"])
def test_the_options_table_lists_exactly_the_plugin_options(path):
    """`--makotest-on` and friends are the session contract."""
    text = _read(path)
    row = re.search(r"\*\*Options\*\*\s*\|([^|]*)\|", text)
    assert row, "no Options row in the doc's table"
    listed = set(re.findall(r"`(--[a-z-]+)", row.group(1)))

    parser = _plugin_option_names()
    assert listed == parser


def _plugin_option_names() -> set[str]:
    """The `--…` flags `pytest_addoption` registers, without running pytest."""
    recorded: set[str] = set()

    class _Group:
        def addoption(self, name, **_):
            recorded.add(name)

    class _Parser:
        def getgroup(self, *_args, **_kw):
            return _Group()

    plugin.pytest_addoption(_Parser())  # type: ignore[arg-type]
    return recorded


def test_the_strategy_table_lists_exactly_the_public_strategies():
    text = _read(SITE)
    table = text[text.index("| Strategy | Draws |") :]
    listed = set(re.findall(r"\| `([a-z_]+)\(", table))
    assert listed == set(strategies.__all__)


def test_the_site_documents_every_cli_subcommand():
    """A subcommand nobody documents is one nobody runs."""
    text = _read(SITE)
    subcommands = {
        choice
        for action in cli._parser()._actions
        if getattr(action, "choices", None)
        for choice in action.choices
    }
    documented = set(re.findall(r"makotest ([a-z]+)\b", text))
    assert not subcommands - documented


def test_the_binding_boundary_table_names_only_real_crates():
    """The table claims which Rust crate each concern comes from.

    A crate named there that the wheel does not link is a claim about
    provenance that is simply false — and provenance is the whole argument for
    the toolkit binding rather than reimplementing.
    """
    linked = set(
        re.findall(
            r"^([a-z0-9-]+)\s*=",
            (_REPO / "makotest/Cargo.toml").read_text(encoding="utf-8"),
            re.MULTILINE,
        )
    )
    for path in (SITE, README):
        text = _read(path)
        claimed = set(re.findall(r"Rust\s+[—-]?\s*`([a-z0-9-]+)(?:::[a-z_:]+)?`", text))
        unknown = {c for c in claimed if c not in linked}
        assert not unknown, f"{path.name} credits unlinked crate(s) {sorted(unknown)}"
