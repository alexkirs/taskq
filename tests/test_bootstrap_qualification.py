"""#177 bounded qualification: network versus login. Fixtures only for failures; no live worker, credential or
network write."""
import importlib
import sys
import unittest
from unittest.mock import Mock, patch

doctor = importlib.import_module('taskq.doctor')


class NetworkOrLogin(unittest.TestCase):
    """A failed authorized read is shown exactly; nothing is written."""
    def gap(self, reply):
        with patch.object(doctor, 'api_read', Mock(return_value=reply)) as read:
            found = doctor.login_gap('gh', None)
        read.assert_called_once_with(['gh', 'api', 'user'])
        return found

    def test_read_succeeds_no_gap(self):
        self.assertIsNone(self.gap((0, '')))  # `auth status` alone is not a diagnosis

    def test_failure_is_printed_as_is(self):
        for stderr in ('error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com',
                       'gh: Bad credentials (HTTP 401)',
                       'gh: Resource not accessible by integration (HTTP 403)'):
            with self.subTest(stderr=stderr):
                what, fix = self.gap((1, stderr))
                self.assertEqual((what, fix), (stderr.splitlines()[-1], 'gh api user'))
        self.assertEqual(self.gap((1, ''))[0], 'gh api user failed without error output')

    def test_api_read_is_read_only_and_survives_a_missing_cli(self):
        self.assertEqual(doctor.api_read(['taskq-no-such-cli-177', 'api', 'user'])[0], 1)
        self.assertEqual(doctor.api_read([sys.executable, '-c', 'import sys; sys.exit(0)']), (0, ''))


if __name__ == '__main__':
    unittest.main()
