"""Checks the README's Python against the API it documents.

`docs/migrating.md` is executed end to end by `test_docs_snippets.py`, because it
was written to be read top-to-bottom and its snippets define their own inputs.
The README is not like that: its blocks are illustrative and reference a `payload`
the reader supplies, so executing them wholesale is not possible.

What *is* possible is catching the drift that actually hurts. A README that
imports a name the package does not export, or documents a `mode` it does not
accept, or lists a warning kind the parser never emits, is worse than one that is
merely terse -- and nothing checked any of those. Three surfaces were added to it
in a single day (metadata mode, the tree, the warnings channel), which is exactly
when that goes wrong.
"""
import ast
import pathlib
import re

import pytest

import fast_mail_parser
from fast_mail_parser import parse_email

README = pathlib.Path(__file__).resolve().parent.parent / "Readme.md"
SOURCE = pathlib.Path(__file__).resolve().parent.parent / "src" / "mail_parser.rs"

MESSAGE = (
    b"From: sender@example.com\r\n"
    b"Subject: readme\r\n"
    b"Content-Type: text/plain\r\n"
    b"\r\n"
    b"body\r\n"
)


def _blocks() -> list[str]:
    text = README.read_text(encoding="utf-8")
    blocks = re.findall(r"```python\n(.*?)```", text, re.S)
    # The rename banner at the top of the README is a blockquote, so its fenced
    # block arrives with `> ` on every line and is not valid Python until that is
    # removed. Found by running this check before committing it.
    return [re.sub(r"(?m)^> ?", "", block) for block in blocks]


def test__the_readme_has_python_blocks():
    # If this drops to zero the rest of the file silently stops testing anything.
    assert len(_blocks()) >= 5, len(_blocks())


@pytest.mark.parametrize("index", range(len(_blocks())))
def test__every_block_parses(index: int):
    source = _blocks()[index]
    try:
        ast.parse(source)
    except SyntaxError as error:
        pytest.fail(f"README block {index} is not valid Python: {error}\n{source}")


def test__every_name_the_readme_imports_exists():
    missing = []
    for index, source in enumerate(_blocks()):
        for node in ast.walk(ast.parse(source)):
            if not isinstance(node, ast.ImportFrom):
                continue
            if node.module != "fast_mail_parser":
                continue
            for alias in node.names:
                if not hasattr(fast_mail_parser, alias.name):
                    missing.append(f"block {index}: {alias.name}")

    assert not missing, "the README imports names the package does not export: " + ", ".join(
        missing
    )


def test__every_documented_mode_is_accepted():
    # Pulled from the README's own prose and code rather than hardcoded, so a mode
    # documented but never implemented fails here.
    text = README.read_text(encoding="utf-8")
    modes = set(re.findall(r'mode="([a-z]+)"', text))

    assert modes, "no modes documented in the README"
    for mode in sorted(modes):
        # Raises ValueError for an unknown mode, which is the failure being caught.
        parse_email(MESSAGE, mode=mode)


def test__every_documented_warning_kind_exists_in_the_source():
    # The README tabulates warning kinds twice (what each repairs, and what strict
    # raises). Both rot silently if a kind is renamed in Rust, and no test read
    # them until now.
    # Scoped to the warnings section. Matching every backticked first cell in the
    # README swept up attribute tables -- `headers`, `content`, `mimetype` -- and
    # reported them as undefined warning kinds.
    readme = README.read_text(encoding="utf-8")
    section = re.search(
        r"### Parse warnings.*?(?=\n## )", readme, re.S
    )
    assert section, "could not find the parse-warnings section in the README"

    # Hyphenated only: the same pattern otherwise picks up the tables' own header
    # cells (`kind`, `detail`). Every kind the parser defines is hyphenated, and
    # if one ever is not, the reverse check below fails loudly rather than
    # quietly skipping it.
    documented = {
        cell
        for cell in re.findall(r"^\| `([a-z-]+)` \|", section.group(0), re.M)
        if "-" in cell
    }
    in_source = set(
        re.findall(r'KIND_[A-Z_]+: &str = "([a-z-]+)"', SOURCE.read_text(encoding="utf-8"))
    )

    assert in_source, "no KIND_ constants found; the parsing above has drifted"
    unknown = documented - in_source
    assert not unknown, f"README documents warning kinds the source does not define: {unknown}"

    undocumented = in_source - documented
    assert not undocumented, f"the source emits kinds the README does not document: {undocumented}"
