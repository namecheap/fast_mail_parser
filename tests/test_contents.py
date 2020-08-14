from fast_mail_parser import PyMail


def test__plain_text_is_available(valid_mail: PyMail):
    assert len(valid_mail.text_plain) is 1


def test__plain_text_is_correct(valid_mail: PyMail):
    assert 'Check out the stream calendar & follow us on Twitch' in valid_mail.text_plain[0]


def test__html_is_available(valid_mail: PyMail):
    assert len(valid_mail.text_html) is 1


def test__html_is_correct(valid_mail: PyMail):
    assert '<!doctype html>' in valid_mail.text_html[0]
