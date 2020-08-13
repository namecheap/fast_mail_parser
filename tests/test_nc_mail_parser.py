import pytest

from nc_mail_parser import parse_email, ParseError


class TestNcMailParser:
    @pytest.fixture
    def valid_message(self) -> str:
        with open('tests/data/valid_message.eml', 'r') as f:
            return f.read()

    @pytest.fixture
    def invalid_message(self) -> str:
        with open('tests/data/invalid_message.eml', 'r') as f:
            return f.read()

    def test__parse_valid_message__plain_text_is_on_place(self, valid_message: str):
        email = parse_email(valid_message)

        assert email.text_plain[0].index('View this email in your browser')

    def test__parse_valid_message__html_text_is_on_place(self, valid_message: str):
        email = parse_email(valid_message)

        assert email.text_html[0].index('<head>')

    def test__parse_valid_message__headers_is_on_place(self, valid_message: str):
        email = parse_email(valid_message)

        assert len(email.headers) is 29

    def test__parse_invalid_message__valid_error_raised(self, invalid_message: str):
        with pytest.raises(ParseError):
            parse_email(invalid_message)
