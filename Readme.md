# fast_mail_parser
![Test](https://github.com/namecheap/fast_mail_parser/workflows/Test/badge.svg)
[![PyPI version](https://badge.fury.io/py/fast-mail-parser.svg)](https://badge.fury.io/py/fast-mail-parser)

fast_mail_parser is a Python library for .eml files parsing.
The main benefit is a performance: the library is much faster than python implementations.

Based on [mailparse](https://github.com/staktrace/mailparse) library using [pyo3](https://github.com/PyO3/pyo3).

## Benchmark

```
============================= test session starts ==============================
platform linux -- Python 3.8.5, pytest-6.0.1
benchmark: 3.2.3 (defaults: timer=time.perf_counter disable_gc=False min_rounds=5 min_time=0.000005 max_time=1.0 calibration_precision=10 warmup=False warmup_iterations=100000)

Name (time in ms)                              Min                Max               Mean            StdDev             Median               IQR            Outliers       OPS            Rounds  Iterations
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
test__fast_mail_parser___parse_message      1.7381 (1.0)      10.2716 (1.0)       2.0991 (1.0)      0.4700 (1.0)       2.0442 (1.0)      0.2377 (1.0)         13;17  476.3975 (1.0)         403           1
test__mail_parser___parse_message          15.1113 (8.69)     18.5801 (1.81)     16.0706 (7.66)     0.8949 (1.90)     15.7256 (7.69)     0.7293 (3.07)          2;1   62.2253 (0.13)         12           1
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

Legend:
  Outliers: 1 Standard Deviation from Mean; 1.5 IQR (InterQuartile Range) from 1st Quartile and 3rd Quartile.
  OPS: Operations Per Second, computed as 1 / Mean
```


## Installation

Use the package manager [pip](https://pypi.org/project/fast_mail_parser/) to install fast_mail_parser.

```bash
pip install fast-mail-parser
```

## Usage

```python
import sys
from fast_mail_parser import parse_email, ParseError

with open('message.eml', 'r') as f:
    message_payload = f.read()

try:
    email = parse_email(message_payload)
except ParseError as e:
    print("Failed to parse email: ", e)
    sys.exit(1)

print(email.subject)
print(email.date)
print(email.text_plain)
print(email.text_html)
print(email.headers)

for attachment in email.attachments:
    print(attachment.mimetype)
    print(attachment.content)

```

## Contributing
Pull requests are welcome. For major changes, please open an issue first to discuss what you would like to change.

Please make sure to update tests as appropriate.
