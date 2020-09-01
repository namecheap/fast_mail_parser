import sys

import os
import toml
from setuptools import setup
from setuptools.command.test import test as TestCommand

try:
    from setuptools_rust import RustExtension
except ImportError:
    import subprocess

    errno = subprocess.call([sys.executable, "-m", "pip", "install", "setuptools-rust"])
    if errno:
        print("Please install setuptools-rust package")
        raise SystemExit(errno)
    else:
        from setuptools_rust import RustExtension


class PyTest(TestCommand):
    user_options = []

    def run(self):
        import subprocess

        subprocess.check_call(["pytest", "tests", "-s", "-v"])


with open("Cargo.toml") as fp:
    version = toml.load(fp)["package"]["version"]

current = os.path.realpath(os.path.dirname(__file__))
with open(os.path.join(current, 'README.md'), encoding="utf-8") as f:
    long_description = f.read()

setup_requires = ["setuptools-rust>=0.10.1", "wheel"]
install_requires = ["toml~=0.10.0"]
tests_require = install_requires + ["pytest", "pytest-benchmark", "mail-parser"]

setup(
    name="fast_mail_parser",
    version=version,
    packages=["fast_mail_parser"],
    rust_extensions=[RustExtension("fast_mail_parser.fast_mail_parser", debug=False)],
    install_requires=install_requires,
    tests_require=tests_require,
    setup_requires=setup_requires,
    include_package_data=True,
    zip_safe=False,
    cmdclass={"test": PyTest},

    description="Very fast Python library for .eml files parsing.",
    platforms=["Linux"],
    keywords=['mail', 'email', 'parser', 'wrapper'],
    license="Apache License, Version 2.0",
    url="https://github.com/namecheap/fast_mail_parser",
    long_description=long_description,
    long_description_content_type="text/markdown",

    classifiers=[
        "License :: OSI Approved :: Apache Software License",
        "Intended Audience :: Developers",
        "Operating System :: OS Independent",
        "Programming Language :: Python",
        "Programming Language :: Python :: 3.5",
        "Programming Language :: Python :: 3.6",
        "Programming Language :: Python :: 3.7",
    ],
)
