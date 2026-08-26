"""Invariants the project relies on but never checked.

Both of these are silent when broken: nothing raises, no test elsewhere notices,
and the damage shows up outside CI -- in a consumer's type checker, or in a
fixture that no longer matches the generator that is supposed to reproduce it.
"""

import functools
import importlib.util
import pathlib

import pytest

import fast_mail_parser as runtime

REPO = pathlib.Path(__file__).resolve().parent.parent
RFC_DIR = REPO / "tests" / "data" / "rfc"
GENERATOR = REPO / "tests" / "generate_rfc_corpus.py"


# --- PEP 561: the stub is only honoured if py.typed ships --------------------


def test__py_typed_ships_with_the_installed_package():
    # Without this marker, PEP 561 says type checkers must ignore __init__.pyi
    # entirely -- so the stub, and the test that keeps it honest, would both be
    # checking something no consumer ever sees. Asserted against the *installed*
    # package, since that is what gets shipped.
    installed = pathlib.Path(runtime.__file__).parent

    assert (installed / "py.typed").is_file(), (
        f"py.typed missing from the installed package at {installed}; "
        "type checkers will ignore __init__.pyi"
    )


def test__the_stub_ships_with_the_installed_package():
    installed = pathlib.Path(runtime.__file__).parent

    assert (installed / "__init__.pyi").is_file(), (
        "__init__.pyi is not packaged, so consumers get no types at all"
    )


# --- the RFC corpus must still be what its generator produces ----------------


@functools.lru_cache(maxsize=1)
def _load_generator():
    # Loaded by path rather than imported: conftest.py pops the rootdir from
    # sys.path, so `import generate_rfc_corpus` is not reliable here.
    spec = importlib.util.spec_from_file_location("generate_rfc_corpus", GENERATOR)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("name", sorted(_load_generator().BUILDERS))
def test__committed_fixture_matches_its_generator(name: str):
    # generate_rfc_corpus.py promises deterministic output so the corpus can be
    # regenerated and reviewed as a diff. That only holds if nobody has
    # hand-edited a fixture in the meantime, which nothing was checking.
    builder = _load_generator().BUILDERS[name]
    path = RFC_DIR / f"{name}.eml"

    assert path.is_file(), f"{name} is in BUILDERS but has no committed fixture"
    assert path.read_bytes() == builder().as_bytes(), (
        f"{name}.eml differs from what generate_rfc_corpus.py produces -- either "
        "it was hand-edited, or the generator changed without the corpus being "
        "regenerated"
    )


def test__every_committed_fixture_has_a_builder():
    committed = {path.stem for path in RFC_DIR.glob("*.eml")}
    builders = set(_load_generator().BUILDERS)

    assert committed == builders, (
        f"orphaned fixtures {sorted(committed - builders)}, "
        f"builders with no fixture {sorted(builders - committed)}"
    )
