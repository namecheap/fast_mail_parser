# fast_mail_parser
![Test](https://github.com/namecheap/fast_mail_parser/workflows/Test/badge.svg)
[![PyPI version](https://badge.fury.io/py/fast-mail-parser.svg)](https://badge.fury.io/py/fast-mail-parser)
[![Downloads](https://pepy.tech/badge/fast-mail-parser)](https://pepy.tech/project/fast-mail-parser)

fast_mail_parser is a Python library for .eml files parsing.
The main benefit is a performance: the library is much faster than python implementations.

Based on [mailparse](https://github.com/staktrace/mailparse) library using [pyo3](https://github.com/PyO3/pyo3).

## Benchmark

```
 -------------------------------------------------------------------------------------------- benchmark: 2 tests -------------------------------------------------------------------------------------------
Name (time in ms)                              Min                Max               Mean            StdDev             Median               IQR            Outliers       OPS            Rounds  Iterations
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
test__fast_mail_parser___parse_message      1.8136 (1.0)       1.8938 (1.0)       1.8426 (1.0)      0.0176 (1.0)       1.8465 (1.0)      0.0277 (1.0)         180;0  542.7141 (1.0)         450           1
test__mail_parser___parse_message          14.5583 (8.03)     15.8571 (8.37)     15.0264 (8.16)     0.2368 (13.49)    14.9702 (8.11)     0.2887 (10.42)         5;1   66.5495 (0.12)         32           1
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
    print(attachment.filename)
```

## Contributing
Pull requests are welcome. For major changes, please open an issue first to discuss what you would like to change.

Please make sure to update tests as appropriate.
