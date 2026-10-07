"""Compare site inputs against successful Pages deployment metadata; unknown baselines build fully."""
import os
import json
import re
import urllib.request
import subprocess

# Jekyll's source, config and dependencies are inside docs; its action is configured here.
INPUTS = ('docs', '.github/workflows/pages.yml', '.github/scripts/pages_inputs.py')


def deployed_baseline(repo):
    """Only a successful latest Pages deployment is usable; pending/failed/unknown builds fully.

    Public read-only metadata needs no token or additional workflow permissions.
    """
    # ponytail: public metadata only; an approved read path is needed to optimize private repos.
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*', repo):
        return ''

    def read(path):
        request = urllib.request.Request(f'https://api.github.com/repos/{repo}/{path}',
                                         headers={'Accept': 'application/vnd.github+json', 'Cache-Control': 'no-cache'})
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)

    try:
        deployment = read('deployments?environment=github-pages&per_page=1')[0]
        if deployment['environment'] != 'github-pages' or not re.fullmatch(r'[0-9a-f]{40}', deployment['sha']):
            return ''
        if not isinstance(deployment['id'], int) or deployment['id'] <= 0:
            return ''
        status = read(f'deployments/{deployment["id"]}/statuses?per_page=1')[0]
        return deployment['sha'] if status['state'] == 'success' else ''
    except (OSError, ValueError, KeyError, IndexError, TypeError):
        return ''


def publication_reason(before, after, event):
    if event == 'workflow_dispatch':
        return 'manual full build'
    if not before or set(before) == {'0'}:
        return 'unknown deployment baseline: full build'
    if subprocess.run(['git', 'cat-file', '-e', f'{before}:.github/workflows/pages.yml'],
                      capture_output=True).returncode:
        return 'unknown or pre-migration baseline: full build'
    result = subprocess.run(['git', 'diff', '--quiet', '--no-renames', before, after, '--', *INPUTS], capture_output=True)
    return ('site inputs unchanged' if result.returncode == 0 else
            'site inputs changed' if result.returncode == 1 else 'unreadable diff: full build')


if __name__ == '__main__':
    baseline = deployed_baseline(os.environ['GITHUB_REPOSITORY'])
    reason = publication_reason(baseline, os.environ['AFTER'], os.environ['EVENT'])
    receipt = f'Pages build decision for `{os.environ["AFTER"]}` against deployment `{baseline or "unknown"}`: {reason}.'
    print(receipt)
    publish = reason != 'site inputs unchanged'
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write(f'publish={str(publish).lower()}\n')
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
        summary.write(receipt + '\n')
