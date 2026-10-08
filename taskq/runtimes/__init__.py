"""The six operations a taskq runtime supports."""
import os
import re
from importlib import import_module

# #268: the orchestrator that launched a session, by its runtime; a name carries it after `T<N>`/`S<N>`.
ORCH = {'claude': 'CLD', 'codex': 'CDX', 'dot': 'DOT', 'hermes': 'HRM', 'grok': 'GRK'}
NAMED = re.compile(rf'^([TS]\d+) (?:({"|".join([*ORCH.values(), "UNK"])}) )?')


def get(name, **options):
    return import_module(f'taskq.runtimes.{name}').Adapter(**options)


def launcher():
    """The runtime of the calling session: TASKQ_RUNTIME, else the session variable it inherits; None: unknown."""
    from taskq import RUNTIMES
    return os.environ.get('TASKQ_RUNTIME') or next((runtime for runtime, variable in RUNTIMES.items() if os.environ.get(variable)), None)


def session_name(name, machine, runtime=None):
    """#268: `<T|S><N> <ORCH> <title> (<machine>)`. A name without `T<N>`/`S<N>` gets only the machine; one that
    already carries an ORCH keeps it."""
    found = NAMED.match(name)
    if found and not found[2]:
        name = f'{found[1]} {ORCH.get(runtime or launcher(), "UNK")} {name[found.end():]}'
    return name if name.endswith(f' ({machine})') else f'{name} ({machine})'


def orchestrator(name):
    """The ORCH code of a session name, or None: an old `T<N> <title> (<machine>)` name has none."""
    found = NAMED.match(name or '')
    return found and found[2]
