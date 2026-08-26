"""Checks that `__init__.pyi` describes the extension it ships with.

`mypy --strict` verifies the stub is internally consistent, and
`tests/test_contract.py` freezes the *runtime* surface. Neither compares the two.
So a stub that omits a real attribute, or declares one that does not exist, is
invisible here while every consumer's type checker sees it -- the failure lands
on users rather than on CI.

This closes that gap by reading the stub as a syntax tree and comparing what it
declares against the extension actually loaded.
"""

import ast
import os

import pytest

import fast_mail_parser as runtime

STUB = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "fast_mail_parser",
    "__init__.pyi",
)


def _parse_stub() -> ast.Module:
    with open(STUB, encoding="utf-8") as handle:
        return ast.parse(handle.read(), filename=STUB)


STUB_TREE = _parse_stub()


def _stub_all() -> set[str]:
    for node in STUB_TREE.body:
        if isinstance(node, ast.Assign) and any(
            getattr(target, "id", None) == "__all__" for target in node.targets
        ):
            return {element.value for element in node.value.elts}
    raise AssertionError("the stub declares no __all__")


def _stub_functions() -> set[str]:
    return {n.name for n in STUB_TREE.body if isinstance(n, ast.FunctionDef)}


def _stub_classes() -> dict[str, ast.ClassDef]:
    return {n.name: n for n in STUB_TREE.body if isinstance(n, ast.ClassDef)}


def _declared_attributes(cls: ast.ClassDef) -> set[str]:
    """Attribute names the stub gives a class: `self.x = ...` plus @property."""
    attributes = set()
    for member in cls.body:
        if not isinstance(member, ast.FunctionDef):
            continue
        decorators = {d.id for d in member.decorator_list if isinstance(d, ast.Name)}
        if "property" in decorators:
            attributes.add(member.name)
        elif member.name == "__init__":
            for node in ast.walk(member):
                if not isinstance(node, ast.Assign):
                    continue
                for target in node.targets:
                    if (
                        isinstance(target, ast.Attribute)
                        and isinstance(target.value, ast.Name)
                        and target.value.id == "self"
                    ):
                        attributes.add(target.attr)
    return attributes


def _public_attributes(obj: type) -> set[str]:
    return {name for name in dir(obj) if not name.startswith("_")}


def test__stub_and_runtime_export_the_same_names():
    assert _stub_all() == set(runtime.__all__)


def test__every_name_the_stub_declares_exists_at_runtime():
    declared = _stub_functions() | set(_stub_classes())

    missing = sorted(name for name in declared if not hasattr(runtime, name))

    assert not missing, f"declared in the stub but absent from the extension: {missing}"


def test__every_exported_name_is_declared_in_the_stub():
    # The direction that bites users: a real attribute the stub forgot, so their
    # type checker rejects working code.
    declared = _stub_functions() | set(_stub_classes())

    undeclared = sorted(set(runtime.__all__) - declared)

    assert not undeclared, f"exported but undeclared in the stub: {undeclared}"


@pytest.mark.parametrize(
    "class_name",
    sorted(
        name
        for name, node in _stub_classes().items()
        if not issubclass(getattr(runtime, name, object), BaseException)
    ),
)
def test__stub_attributes_match_the_runtime_class(class_name: str):
    stub_attributes = _declared_attributes(_stub_classes()[class_name])
    runtime_attributes = _public_attributes(getattr(runtime, class_name))

    assert stub_attributes == runtime_attributes, (
        f"{class_name}: stub-only {sorted(stub_attributes - runtime_attributes)}, "
        f"runtime-only {sorted(runtime_attributes - stub_attributes)}"
    )


@pytest.mark.parametrize(
    "class_name",
    sorted(
        name
        for name in _stub_classes()
        if issubclass(getattr(runtime, name, object), BaseException)
    ),
)
def test__stub_exception_bases_match_the_runtime_hierarchy(class_name: str):
    node = _stub_classes()[class_name]
    declared_bases = {b.id for b in node.bases if isinstance(b, ast.Name)}
    actual = getattr(runtime, class_name)

    for base in declared_bases:
        if base == "Exception":
            assert issubclass(actual, Exception)
        else:
            assert issubclass(actual, getattr(runtime, base)), (
                f"the stub says {class_name}({base}) but the runtime disagrees"
            )
