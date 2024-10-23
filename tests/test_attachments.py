import typing as t
from fast_mail_parser import PyMail


def test__attachments_are_available(attachment_mail: PyMail):
    assert len(attachment_mail.attachments) == 4


def test__base64_content_is_decoded(attachment_mail: PyMail):
    attachment = list(filter(lambda a: a.mimetype == 'image/png', attachment_mail.attachments)).pop()

    assert attachment.content == b'PNG here'


def test__expected_attachments_are_present(large_mail: PyMail):
    expected_attachment_names: t.Set[str] = {'Lorem Ipsum - All the facts.pdf', 'Kitty Dark.png'}
    attachments = [a for a in large_mail.attachments if a.filename in expected_attachment_names]

    assert len(attachments) == 2
