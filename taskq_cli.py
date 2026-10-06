"""The `taskq` command: imports the package, and names a broken one in one line instead of a traceback.
An editable install runs its clone's working tree, so a half-done edit there breaks every session on the machine."""
from importlib.util import find_spec
from pathlib import Path
import sys


def main():
    try:
        from taskq import main as run
    except Exception as error:
        spec = find_spec('taskq')  # finds the package without running it
        where = Path(spec.origin).resolve().parents[1] if spec and spec.origin else 'unknown'
        sys.exit(f'taskq is broken at {where}: {type(error).__name__}: {error}; run `git -C {where} status`')
    return run()
