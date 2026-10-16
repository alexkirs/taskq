"""Release contract: stale, foreign and wrong-workflow success cannot publish."""
from pathlib import Path
import runpy

select = runpy.run_path(str(Path(__file__).with_name('release-check.py')))['selected']
base = dict(id=1, head_branch='main', head_sha='a'*40, path='.github/workflows/tests.yml', event='push', created_at='2026-10-17T00:00:00Z', status='completed', conclusion='success')
now = 1792198800  # 2026-10-17T01:00:00Z
assert select([base], 'a'*40, 'main', now) == base
for edit in [dict(head_sha='b'*40), dict(head_branch='another-branch'), dict(path='.github/workflows/other.yml'), dict(event='workflow_run'), dict(event='pull_request'), dict(created_at='bad'), dict(created_at='2026-10-15T00:00:00Z'), dict(created_at='2026-10-18T00:00:00Z')]:
    assert select([{**base, **edit}], 'a'*40, 'main', now) is None, edit
failed = {**base, 'id':2, 'conclusion':'failure'}
assert select([base, failed], 'a'*40, 'main', now) == failed, 'newest failure cannot reuse earlier success'
pending = {**base, 'id':3, 'status':'in_progress', 'conclusion':None}
assert select([base, failed, pending], 'a'*40, 'main', now) == pending
assert select([{**base, 'head_branch':'v9'}], 'a'*40, 'main', now) is None
print('PASS exact-source release check, freshness and latest failure/pending isolation')
