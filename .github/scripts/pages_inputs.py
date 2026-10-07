"""Compare all site inputs across a push/PR; unknown or initial baselines build fully."""
import os
import subprocess

# Jekyll's source, config and dependencies are inside docs; its action is configured here.
INPUTS = ('docs', '.github/workflows/pages.yml', '.github/scripts/pages_inputs.py')


def publication_reason(before, after, event):
    if event == 'workflow_dispatch':
        return 'manual full build'
    if not before or set(before) == {'0'}:
        return 'initial full build'
    if subprocess.run(['git', 'cat-file', '-e', f'{before}:.github/workflows/pages.yml'],
                      capture_output=True).returncode:
        return 'unknown or pre-migration baseline: full build'
    result = subprocess.run(['git', 'diff', '--quiet', '--no-renames', before, after, '--', *INPUTS], capture_output=True)
    return ('site inputs unchanged' if result.returncode == 0 else
            'site inputs changed' if result.returncode == 1 else 'unreadable diff: full build')


if __name__ == '__main__':
    reason = publication_reason(os.environ.get('BEFORE', ''), os.environ['AFTER'], os.environ['EVENT'])
    publish = reason != 'site inputs unchanged'
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write(f'publish={str(publish).lower()}\n')
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
        summary.write(f'Pages build decision for `{os.environ["AFTER"]}`: {reason}.\n')
