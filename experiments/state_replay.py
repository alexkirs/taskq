"""#641 pure replay of the actual lifecycle reducer, never board authority.

stdin: {"raw": native task payload, "commands": [{"transition": ..., "facts": ...}]}
Adapter facts in a trace are abstract observations, not native process/drain proof.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from taskq import lifecycle_transition


def replay(raw, commands):
    outcomes = []
    for command in commands:
        try:
            raw = lifecycle_transition(raw, command['transition'], command['facts'])
            outcomes.append('ok')
        except ValueError as error:
            outcomes.append(str(error))
    return {'raw': raw, 'outcomes': outcomes}


if __name__ == '__main__':
    trace = json.load(sys.stdin)
    print(json.dumps(replay(trace['raw'], trace['commands']), sort_keys=True))
