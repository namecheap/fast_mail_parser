import threading

from fast_mail_parser import PyMail, parse_email


def test__attachments_are_available(attachment_mail: PyMail):
    # The fixture is a multipart/mixed with an HTML body, an inline PNG, and a
    # plain-text body. Only the PNG is an attachment: the container is MIME
    # structure and the two text parts are bodies.
    assert len(attachment_mail.attachments) == 1

    attachment = attachment_mail.attachments[0]
    assert attachment.mimetype == "image/png"
    assert attachment.filename == "image.png"


def test__body_parts_are_not_attachments(attachment_mail: PyMail):
    # The same fixture's text parts reach the body lists, not attachments.
    assert len(attachment_mail.text_html) == 1
    assert "HTML here" in attachment_mail.text_html[0]

    assert len(attachment_mail.text_plain) == 1
    assert "Plaintext here." in attachment_mail.text_plain[0]


def test__base64_content_is_decoded(attachment_mail: PyMail):
    attachment = list(
        filter(lambda a: a.mimetype == 'image/png', attachment_mail.attachments)
    ).pop()

    assert attachment.content == b'PNG here'


def test__expected_attachments_are_present(large_mail: PyMail):
    expected_attachment_names: set[str] = {'Lorem Ipsum - All the facts.pdf', 'Kitty Dark.png'}
    attachments = [a for a in large_mail.attachments if a.filename in expected_attachment_names]

    assert len(attachments) == 2


def test__content_id_is_none_when_absent(attachment_mail: PyMail):
    # The fixture's PNG declares a disposition but no Content-ID.
    attachment = attachment_mail.attachments[0]

    assert attachment.content_id is None
    assert attachment.disposition == "inline"


def test__disposition_is_none_when_the_header_is_absent():
    # An absent Content-Disposition is reported distinctly from an explicit
    # `inline` -- mailparse defaults its parsed disposition to Inline, so the two
    # would otherwise be indistinguishable.
    raw = (
        b"Subject: no disposition\r\n"
        b"MIME-Version: 1.0\r\n"
        b'Content-Type: multipart/mixed; boundary="b1"\r\n'
        b"\r\n"
        b"--b1\r\n"
        b"Content-Type: text/plain\r\n"
        b"\r\n"
        b"body\r\n"
        b"--b1\r\n"
        b"Content-Type: image/png\r\n"
        b"Content-Transfer-Encoding: base64\r\n"
        b"\r\n"
        b"UE5HIGhlcmU=\r\n"
        b"--b1--\r\n"
    )
    mail = parse_email(raw)

    png = next(a for a in mail.attachments if a.mimetype == "image/png")
    assert png.disposition is None
    assert png.content_id is None
    assert png.content == b"PNG here"


def test__repeated_content_reads_return_the_same_object(attachment_message: str):
    # #227: the getter used to build a fresh `bytes` per read, so this was False
    # and every read copied the decoded payload again. Lazy mode has guaranteed
    # this since #97; full mode now matches it.
    mail = parse_email(attachment_message)
    attachment = mail.attachments[0]

    first = attachment.content

    assert attachment.content is first
    assert attachment.content is first


def test__the_attachment_list_hands_back_the_same_objects(attachment_message: str):
    # Reading `attachments` builds a new list, but of the same parts. If it built
    # new parts -- which is what PyO3's clone path for a `Vec<pyclass>` field did
    # -- each read would get a fresh cache and nothing above would ever be
    # cached, so this is what makes that test mean anything.
    mail = parse_email(attachment_message)

    assert mail.attachments[0] is mail.attachments[0]

    first = mail.attachments[0].content
    assert mail.attachments[0].content is first


def test__concurrent_first_content_reads_share_one_object(large_message: str):
    # The `OnceLock` publishes once; racing first readers must all come back with
    # that one object rather than each building their own. Mirrors the lazy
    # mode's test of the same hazard.
    mail = parse_email(large_message)
    attachment = max(mail.attachments, key=lambda a: len(a.content))
    expected = max(parse_email(large_message).attachments, key=lambda a: len(a.content)).content

    workers = 16
    start = threading.Barrier(workers)
    seen: list[bytes] = []
    failures: list[BaseException] = []
    lock = threading.Lock()

    def hammer() -> None:
        try:
            start.wait()
            for _ in range(20):
                content = attachment.content
                with lock:
                    seen.append(content)
        except BaseException as exc:  # noqa: BLE001 - reported, not swallowed
            with lock:
                failures.append(exc)

    threads = [threading.Thread(target=hammer) for _ in range(workers)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert not failures, failures
    assert len(seen) == workers * 20
    assert all(content is seen[0] for content in seen), "the cache handed out copies"
    assert seen[0] == expected
