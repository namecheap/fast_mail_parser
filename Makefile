.PHONY: install

test:
	pytest -v --ignore tests/benchmark tests

benchmark:
	pytest -v tests/benchmark

install:
	python3 setup.py install --force
