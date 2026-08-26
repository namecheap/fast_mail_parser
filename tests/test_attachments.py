from fast_mail_parser import PyMail


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
