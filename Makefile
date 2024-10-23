.PHONY: install

test:
	pytest -v --ignore tests/benchmark tests

benchmark:
	pytest -v tests/benchmark

install:
	pip install .

install-test:
	pip install ".[test]"
