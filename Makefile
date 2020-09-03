.PHONY: build

test:
	pytest -v --ignore tests/benchmark tests

benchmark:
	pytest -v tests/benchmark

build:
	docker run --rm -v $(CURDIR):/io konstin2/maturin build --release --strip --manylinux 1

publish:
	twine check target/wheels/*
	twine upload target/wheels/*
