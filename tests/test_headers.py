import threading

import pytest

from fast_mail_parser import PyMail, parse_email, parse_email_tree


def test__total_number_is_valid(valid_mail: PyMail):
    assert len(valid_mail.headers) == 29


def test__header_is_accessible_by_key(valid_mail: PyMail):
    assert valid_mail.headers['Reply-To'] == ['Red Hat OpenShift <noreply@openshift.com>']


def test__subject_is_available_via_property(valid_mail: PyMail):
    assert valid_mail.subject == 'Your June OpenShift Update'


def test__date_is_available_via_property(valid_mail: PyMail):
    assert valid_mail.date == 'Wed, 1 Jul 2020 05:33:42 +0000'


# --- the headers dict is built once and shared (#231) -------------------------


def _every_headers_bearer(payload: bytes) -> list:
    """One object of each class that exposes `headers`."""
    tree = parse_email_tree(payload)
    return [
        parse_email(payload),
        parse_email(payload, mode="metadata"),
        parse_email(payload, mode="lazy"),
        tree,
        tree.children[0],
        parse_email_tree(payload, mode="metadata"),
        parse_email_tree(payload, mode="lazy"),
    ]


def test__headers_is_the_same_dict_on_every_read(valid_message: str):
    # Before #231 each read rebuilt the whole dict -- one PyDict, a str per key,
    # a list per key, a str per value. All six classes now publish one dict.
    for obj in _every_headers_bearer(valid_message.encode()):
        assert obj.headers is obj.headers, type(obj).__name__


def test__headers_is_still_a_plain_dict_of_str_to_list(valid_mail: PyMail):
    # Sharing the object must not change what it is: `dict`, not a proxy.
    headers = valid_mail.headers

    assert type(headers) is dict
    assert all(isinstance(key, str) for key in headers)
    assert all(isinstance(value, list) for value in headers.values())
    assert all(isinstance(item, str) for value in headers.values() for item in value)


def test__headers_is_built_on_first_read_and_shared_across_threads(valid_message: str):
    # The cell publishes once; racing first readers must all get that one dict
    # rather than each building their own.
    mail = parse_email(valid_message.encode())

    workers = 8
    start = threading.Barrier(workers)
    seen: list[dict] = []
    failures: list[BaseException] = []
    lock = threading.Lock()

    def read() -> None:
        try:
            start.wait()
            headers = mail.headers
            with lock:
                seen.append(headers)
        except BaseException as exc:  # noqa: BLE001 - reported, not swallowed
            with lock:
                failures.append(exc)

    threads = [threading.Thread(target=read) for _ in range(workers)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert not failures, failures
    assert len({id(headers) for headers in seen}) == 1, "readers got different dicts"


def test__mutating_the_headers_dict_does_not_change_the_parse(valid_message: str):
    # The documented consequence of sharing, pinned: the dict is the same object
    # on later reads, so edits to it persist -- but the parse behind it does not
    # move, and a fresh parse is unaffected. See docs/migrating.md.
    payload = valid_message.encode()
    mail = parse_email(payload)
    subject_before = mail.subject
    original = dict(mail.headers)

    headers = mail.headers
    headers["X-Injected"] = ["1"]
    headers.pop("Subject", None)

    assert mail.subject == subject_before
    assert parse_email(payload).headers == original
    assert mail.headers is headers
    assert "X-Injected" in mail.headers


@pytest.mark.parametrize("mode", ["full", "metadata", "lazy"])
def test__headers_agree_across_modes(valid_message: str, mode: str):
    # Caching must not make one mode's projection drift from another's.
    payload = valid_message.encode()
    expected = parse_email(payload).headers

    actual = parse_email(payload) if mode == "full" else parse_email(payload, mode=mode)

    assert actual.headers == expected
