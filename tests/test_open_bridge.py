"""Run the unchanged session bridge in a JS VM without opening an application."""
import json
from pathlib import Path
import shutil
import subprocess
import unittest


@unittest.skipUnless(shutil.which('node'), 'Node is required to execute the existing JavaScript bridge')
class Bridge(unittest.TestCase):
    def test_schemes_uuid_hash_and_fallback(self):
        page = (Path(__file__).resolve().parents[1] / 'docs/open.html').read_text()
        script = page.split('<script>', 1)[1].split('</script>', 1)[0]
        harness = '''
const vm = require('node:vm'), assert = require('node:assert/strict');
const script = SCRIPT;
const uuid = '01234567-89ab-cdef-0123-456789abcdef';
function run(hash) {
  const out = {textContent:'No session link in this address.', append(link) {this.link=link;}};
  const location = {hash};
  const document = {getElementById() {return out;}, createElement() {return {};}};
  let error;
  try {vm.runInNewContext(script, {location, document});} catch (e) {error=e;}
  return {out, location, error};
}
for (const scheme of ['codex://threads/', 'claude://resume?', 'claude://resume/']) {
  const target = scheme + uuid;
  const result = run('#' + encodeURIComponent(target));
  if (scheme.endsWith('?')) {assert.equal(result.location.href, undefined); continue;}
  assert.equal(result.location.href, target);
  assert.equal(result.out.link.href, target);
  assert.equal(result.out.link.textContent, target);
  assert.equal(result.out.textContent, 'Opening the app. If nothing happens: ');
}
for (const target of ['', 'https://example.com/' + uuid, 'javascript:alert(1)',
                      'codex://threads/not-a-uuid', 'codex://threads/' + uuid + '?extra',
                      'claude://resume/' + uuid + '/extra']) {
  const result = run('#' + encodeURIComponent(target));
  assert.equal(result.location.href, undefined);
  assert.equal(result.out.link, undefined);
  assert.equal(result.out.textContent, 'No session link in this address.');
}
const malformed=run('#%');
assert.equal(malformed.location.href, undefined);
assert.equal(malformed.out.textContent, 'No session link in this address.');
assert.equal(malformed.error.name, 'URIError');
'''.replace('SCRIPT', json.dumps(script))
        subprocess.run(['node', '-e', harness], check=True, capture_output=True, text=True)
        self.assertIn('<meta name="referrer" content="no-referrer">', page)


if __name__ == '__main__':
    unittest.main()
