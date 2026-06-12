#!/usr/bin/env python3
"""Generate the RFC-feature .eml corpus used by tests/test_rfc_corpus.py.

The goal is a stable, checked-in set of messages that exercise the email/MIME
RFCs fast_mail_parser is expected to handle, so the parser's behavior on each
feature is locked against regressions (including across native-binding
upgrades).

Output is deterministic — fixed dates, message-ids, and MIME boundaries — so
re-running this script reproduces byte-identical files (easy to review in diffs
and to regenerate after an intentional change). Run:

    python tests/generate_rfc_corpus.py

then commit the regenerated tests/data/rfc/*.eml files.
"""

import os
from email.message import EmailMessage
from email import policy

OUT_DIR = os.path.join(os.path.dirname(__file__), "data", "rfc")

DATE = "Mon, 01 Jan 2024 12:00:00 +0000"
MSGID = "<fixture@fast-mail-parser.test>"

# A tiny but real PNG (1x1) and PDF-ish blob, so attachments carry binary bytes.
PNG_1x1 = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4"
    "890000000d4944415478da6360000002000154a24f3e0000000049454e44ae42"
    "6082"
)
PDF_BLOB = b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n"


def _base(policy_obj=policy.SMTP):
    msg = EmailMessage(policy=policy_obj)
    msg["From"] = "Sender <sender@example.com>"
    msg["To"] = "Recipient <recipient@example.com>"
    msg["Date"] = DATE
    msg["Message-ID"] = MSGID
    return msg


def _fix_boundaries(msg, tag):
    """Assign deterministic boundaries to every multipart part."""
    index = 0
    for part in msg.walk():
        if part.is_multipart():
            part.set_boundary(f"----={tag}-{index}")
            index += 1
    return msg


def rfc5322_plain():
    # RFC 5322: a minimal text-only message, ASCII, CRLF line endings.
    msg = _base()
    msg["Subject"] = "Plain text message"
    msg.set_content("Hello, this is a simple RFC 5322 message.\n")
    return msg


def multipart_alternative():
    # RFC 2046: multipart/alternative (text/plain + text/html).
    msg = _base()
    msg["Subject"] = "Alternative parts"
    msg.set_content("Plain alternative body.\n")
    msg.add_alternative("<html><body><p>HTML alternative body.</p></body></html>\n",
                        subtype="html")
    return _fix_boundaries(msg, "alt")


def multipart_mixed_attachment():
    # RFC 2046 multipart/mixed with a base64 (RFC 2045) binary attachment.
    msg = _base()
    msg["Subject"] = "Message with attachment"
    msg.set_content("See attached image.\n")
    msg.add_attachment(PNG_1x1, maintype="image", subtype="png", filename="pixel.png")
    return _fix_boundaries(msg, "mixed")


def base64_body():
    # RFC 2045: single-part body with base64 Content-Transfer-Encoding.
    msg = _base()
    msg["Subject"] = "Base64 body"
    msg.set_content("This body is transferred as base64.\n", cte="base64")
    return msg


def quoted_printable_body():
    # RFC 2045: quoted-printable CTE; content has '=' and non-ASCII needing escapes.
    msg = _base()
    msg["Subject"] = "Quoted-printable body"
    msg.set_content("Discount: 50=50? Côté café — déjà vu.\n",
                    charset="utf-8", cte="quoted-printable")
    return msg


def rfc2047_encoded_subject():
    # RFC 2047: non-ASCII Subject serialized as an encoded-word (=?utf-8?...?=).
    msg = _base()
    msg["Subject"] = "Café ☕ — déjà vu update"
    msg.set_content("Body is plain ASCII; the subject is the RFC 2047 part.\n")
    return msg


def rfc2231_param_filename():
    # RFC 2231: attachment with a non-ASCII filename -> filename*=utf-8''... param.
    msg = _base()
    msg["Subject"] = "Attachment with encoded filename"
    msg.set_content("Attachment uses an RFC 2231 encoded filename.\n")
    msg.add_attachment(PDF_BLOB, maintype="application", subtype="pdf",
                       filename="résumé déjà.pdf")
    return _fix_boundaries(msg, "p2231")


def multipart_related():
    # RFC 2387: multipart/related, HTML referencing an inline image by Content-ID.
    msg = _base()
    msg["Subject"] = "Related inline image"
    msg.set_content("<html><body><img src=\"cid:img1\"></body></html>\n", subtype="html")
    msg.add_related(PNG_1x1, maintype="image", subtype="png", cid="<img1>")
    return _fix_boundaries(msg, "rel")


def nested_multipart():
    # multipart/mixed > multipart/alternative (+ attachment): deep MIME tree.
    msg = _base()
    msg["Subject"] = "Nested multipart"
    msg.set_content("Plain part of the alternative.\n")
    msg.add_alternative("<html><body>HTML part of the alternative.</body></html>\n",
                        subtype="html")
    msg.add_attachment(PDF_BLOB, maintype="application", subtype="pdf", filename="doc.pdf")
    return _fix_boundaries(msg, "nest")


def rfc6532_utf8_headers():
    # RFC 6532 / SMTPUTF8 (EAI): raw UTF-8 in headers and an internationalized
    # address, kept literal by the SMTPUTF8 policy (not RFC 2047 encoded-words).
    msg = _base(policy_obj=policy.SMTPUTF8)
    del msg["From"]
    msg["From"] = "Отправитель <отправитель@пример.рф>"
    msg["Subject"] = "Письмо с UTF-8 заголовками"
    msg.set_content("UTF-8 body for an EAI message.\n", charset="utf-8", cte="8bit")
    return msg


def utf8_8bit_body():
    # RFC 6152: 8bit CTE carrying a raw (un-encoded) UTF-8 body.
    msg = _base()
    msg["Subject"] = "8bit UTF-8 body"
    msg.set_content("Ångström café — naïve façade. Юникод. 日本語。\n",
                    charset="utf-8", cte="8bit")
    return msg


def empty_body():
    # Headers only, no body — a valid degenerate message.
    msg = _base()
    msg["Subject"] = "No body"
    msg.set_content("")
    return msg


def folded_header():
    # RFC 5322 folding: a long header value wrapped across continuation lines.
    msg = _base()
    msg["Subject"] = (
        "This is a deliberately long subject line that the email generator must "
        "fold across multiple physical lines using folding whitespace per RFC 5322 "
        "so the parser has to unfold it back into a single logical value"
    )
    msg["References"] = " ".join(f"<ref-{n}@example.com>" for n in range(8))
    msg.set_content("Body after folded headers.\n")
    return msg


BUILDERS = {
    "rfc5322_plain": rfc5322_plain,
    "multipart_alternative": multipart_alternative,
    "multipart_mixed_attachment": multipart_mixed_attachment,
    "base64_body": base64_body,
    "quoted_printable_body": quoted_printable_body,
    "rfc2047_encoded_subject": rfc2047_encoded_subject,
    "rfc2231_param_filename": rfc2231_param_filename,
    "multipart_related": multipart_related,
    "nested_multipart": nested_multipart,
    "rfc6532_utf8_headers": rfc6532_utf8_headers,
    "utf8_8bit_body": utf8_8bit_body,
    "empty_body": empty_body,
    "folded_header": folded_header,
}


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    for name, builder in BUILDERS.items():
        raw = builder().as_bytes()
        path = os.path.join(OUT_DIR, f"{name}.eml")
        with open(path, "wb") as handle:
            handle.write(raw)
        print(f"wrote {path} ({len(raw)} bytes)")


if __name__ == "__main__":
    main()
