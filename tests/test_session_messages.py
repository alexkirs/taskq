"""Shared CLI text input preserves authoring bytes before a route can act."""
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import taskq as q


class TextFileTests(unittest.TestCase):
    def source(self, content='line one\r\nПривет [link](https://example.com)\n`code`\\n'):
        folder = self.enterContext(tempfile.TemporaryDirectory())
        path = Path(folder) / 'message.txt'
        with path.open('w', encoding='utf-8', newline='') as handle:
            handle.write(content)
        return path, content

    def test_file_passes_multiline_unicode_and_literal_escapes_verbatim(self):
        path, expected = self.source()
        with patch.object(q, 'codex_send') as sent:
            q.main(['codex-send', 'thread-1', '--text-file', str(path)])
        self.assertEqual(sent.call_args.args[0].text, expected)

    def test_existing_text_and_optional_spawn_stay_compatible(self):
        with patch.object(q, 'problem') as reported:
            q.main(['problem', '--text', r'literal\n'])
        self.assertEqual(reported.call_args.args[0].text, r'literal\n')
        with patch.object(q, 'spawn') as spawned:
            q.main(['spawn'])
            self.assertIsNone(spawned.call_args.args[0].text)
            path, expected = self.source()
            q.main(['spawn', '--text-file', str(path)])
            self.assertEqual(spawned.call_args.args[0].text, expected)

    def test_required_inputs_reject_absent_or_both_without_dispatch(self):
        path, _ = self.source()
        for argv in (['problem'], ['problem', '--text', 'x', '--text-file', str(path)]):
            with self.subTest(argv=argv), patch.object(q, 'problem') as reported, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                q.main(argv)
            self.assertEqual(error.exception.code, 2)
            reported.assert_not_called()

    def test_unreadable_or_invalid_file_rejects_before_dispatch(self):
        folder = self.enterContext(tempfile.TemporaryDirectory())
        invalid = Path(folder) / 'invalid.txt'
        invalid.write_bytes(b'\xff')
        for path in (Path(folder) / 'missing.txt', Path(folder), invalid):
            with self.subTest(path=path), patch.object(q, 'problem') as reported, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                q.main(['problem', '--text-file', str(path)])
            self.assertEqual(error.exception.code, 2)
            reported.assert_not_called()


if __name__ == '__main__':
    unittest.main()
