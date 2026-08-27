# fast_mail_parser

> ## ⚠️ `fast-mail-parser-ng` is no longer maintained
>
> **Install `fast-mail-parser` instead:**
>
> ```bash
> pip install fast-mail-parser
> ```
>
> **Nothing in your code changes.** The import path has always been
> `fast_mail_parser`, under either distribution name:
>
> ```python
> from fast_mail_parser import parse_email
> ```
>
> So the migration is one line in your requirements file, and nothing else:
>
> ```diff
> - fast-mail-parser-ng
> + fast-mail-parser
> ```

## Why this name existed

`fast-mail-parser-ng` was a temporary home. The original `fast-mail-parser` name
on PyPI belonged to an account this project no longer controlled and was frozen
at an unmaintained 0.2.5 from June 2022; only a project owner can publish to a
name, so fixes could not reach it. Rather than hold releases behind a PEP 541
transfer queue indefinitely, 0.6.0 through 0.7.0 shipped here.

Ownership of the original name has since been transferred directly, so releases
go back to it from **0.8.0** onwards.

## What happens to this project

**0.7.1 is the final release under this name.** Its code is identical to 0.7.0 —
only this notice changed — so upgrading to it gains you nothing but the warning.

The project is being **archived** on PyPI: read-only, taking no further releases.
The versions published here stay installable, so anything pinning `0.6.0`,
`0.6.1`, `0.7.0` or `0.7.1` keeps working. Archiving marks a project finished; it
removes nothing.

## What you are missing by staying

Everything released after 0.7.0 is only on `fast-mail-parser`:

- **`parse_email_tree` + `walk`** — the MIME tree with its structure intact, and
  `message/rfc822` parts parsed rather than opaque
- **`mode="metadata"`** — headers and the attachment inventory with nothing
  decoded, ~3.9x faster
- **`mode="lazy"`** — attachment content decoded on first access and cached
- **`PyMail.warnings` and `strict=True`** — the lossy repairs a parse performed,
  reported instead of silently applied
- **27% faster batch parsing**, wire-order `headers`, a repair for messages
  missing their header/body separator, and internal panics surfacing as
  `ParseError`

## Links

- **Current project:** https://pypi.org/project/fast-mail-parser/
- **Source and issues:** https://github.com/namecheap/fast_mail_parser
- **Changelog:** https://github.com/namecheap/fast_mail_parser/blob/master/CHANGELOG.md

Licensed under Apache-2.0.
