"""The type stubs must match the compiled module, in both directions.

`py.typed` ships with the wheel, so `_native.pyi` is the only thing a consumer's
type checker sees. A stub that drifts is a silent downgrade of every consumer's
type safety, and it drifts the moment a binding is added in Rust without a
matching entry here.
"""

from __future__ import annotations

import ast
import pathlib

from makotest import _native

STUB = pathlib.Path(_native.__file__).with_name("_native.pyi")


def _declared() -> set[str]:
    """Top-level names the stub declares."""
    tree = ast.parse(STUB.read_text(encoding="utf-8"))
    return {
        node.name
        for node in tree.body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))
    }


def _compiled() -> set[str]:
    return {name for name in dir(_native) if not name.startswith("_")}


def test_the_stub_declares_every_compiled_binding():
    missing = _compiled() - _declared()
    assert not missing, (
        f"{sorted(missing)} are exported by the Rust module but absent from "
        f"{STUB.name}. A consumer's type checker cannot see them."
    )


def test_the_stub_declares_nothing_that_does_not_exist():
    extra = _declared() - _compiled()
    assert not extra, (
        f"{sorted(extra)} are declared in {STUB.name} but not exported by the "
        f"Rust module. A consumer would type-check against a function that "
        f"raises ImportError."
    )


def test_the_package_reexports_every_binding_it_documents():
    """`makotest.__all__` must be importable and complete.

    A name in `__all__` that does not exist breaks `from makotest import *`; a
    binding missing from it is invisible to anyone reading the package.
    """
    import makotest

    assert not [n for n in makotest.__all__ if not hasattr(makotest, n)]
    assert makotest.__all__ == sorted(makotest.__all__), "keep __all__ sorted"

    native_only = _compiled() - set(makotest.__all__)
    assert not native_only, (
        f"{sorted(native_only)} are compiled bindings the package does not "
        f"re-export — a consumer would have to reach into makotest._native."
    )
