"""Executes the Python snippets in docs/migrating.md against the built wheel.

A migration guide whose snippets have drifted from the API is worse than no
guide, so the document is the test: every ```python block is extracted and
executed in document order in one shared namespace, exactly as a reader
following the guide top-to-bottom would.

The snippets carry their own assertions about the behaviour being explained, so
this covers both "the code still runs" and "the claims are still true".
"""

import pathlib
import re

import pytest

GUIDE = pathlib.Path(__file__).resolve().parent.parent / "docs" / "migrating.md"


def _python_blocks(text: str) -> list[str]:
    return re.findall(r"```python\n(.*?)```", text, re.S)


def test__guide_exists_and_has_snippets():
    assert GUIDE.is_file(), f"missing {GUIDE}"
    blocks = _python_blocks(GUIDE.read_text(encoding="utf-8"))
    assert len(blocks) >= 5, f"expected several snippets, found {len(blocks)}"


def test__every_snippet_runs_in_document_order():
    # One shared namespace: later snippets build on `mail` from the setup block,
    # which is how the guide reads.
    namespace: dict[str, object] = {}
    for index, source in enumerate(_python_blocks(GUIDE.read_text(encoding="utf-8"))):
        try:
            exec(compile(source, f"docs/migrating.md[block {index}]", "exec"), namespace)
        except Exception as exc:
            pytest.fail(
                f"snippet {index} in docs/migrating.md failed: "
                f"{type(exc).__name__}: {exc}\n---\n{source}"
            )

    # The setup block must have produced the objects the guide talks about.
    assert "mail" in namespace


def test__guide_does_not_document_removed_apis():
    # Guards against the guide describing the pre-0.7.0 shapes it exists to
    # migrate people away from.
    text = GUIDE.read_text(encoding="utf-8")
    for stale in ("dict[str, str]", "multipart/mixed container is reported"):
        assert stale not in text, f"guide still references removed behaviour: {stale}"
