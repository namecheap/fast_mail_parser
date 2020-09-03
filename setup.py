import sys

import toml
from setuptools import setup

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

with open("Cargo.toml") as fp:
    version = toml.load(fp)["package"]["version"]

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
)
