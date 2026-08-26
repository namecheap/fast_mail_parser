.PHONY: install test benchmark bench-table

test:
	pytest -v --ignore tests/benchmark tests

benchmark:
	pytest -v tests/benchmark

# Regenerate the comparison table published in Readme.md. Numbers move with the
# machine, so paste the output together with the methodology line it prints.
bench-table:
	pytest -q tests/benchmark --benchmark-min-rounds=25 --benchmark-json=benchmark.json
	python .github/scripts/bench_table.py benchmark.json

install:
	pip install .

install-test:
	pip install ".[test]"
