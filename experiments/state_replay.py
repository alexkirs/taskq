"""#641 isolated trace replay; no board/process effects or production activation.

Input: JSON command list on stdin. Output includes every refusal and final state.
Revisions are explicit, so stale/concurrent commands replay without wall clocks.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from taskq import pilot_state, pilot_transition


def replay(commands, transition=pilot_transition):
    state, outcomes = pilot_state(), []
    for command in commands:
        try:
            state = transition(state, command)
            outcomes.append('ok')
        except ValueError as error:
            outcomes.append(str(error))
    return {'state': state, 'outcomes': outcomes}


if __name__ == '__main__':
    print(json.dumps(replay(json.load(sys.stdin)), sort_keys=True))
