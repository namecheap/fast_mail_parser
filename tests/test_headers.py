from fast_mail_parser import PyMail


def test__total_number_is_valid(valid_mail: PyMail):
    assert len(valid_mail.headers) is 29


def test__header_is_accessible_by_key(valid_mail: PyMail):
    assert valid_mail.headers['Reply-To'] == 'Red Hat OpenShift <noreply@openshift.com>'


def test__subject_is_available_via_property(valid_mail: PyMail):
    assert valid_mail.subject == 'Your June OpenShift Update'


def test__date_is_available_via_property(valid_mail: PyMail):
    assert valid_mail.date == 'Wed, 1 Jul 2020 05:33:42 +0000'
