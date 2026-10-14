#!/usr/bin/env python3
"""A3 synthetic project, layered over the README snapshot fixture. No board access."""
import json
import pathlib
import subprocess
import sys

root = pathlib.Path(sys.argv[1]).resolve()
subprocess.run(['docs/shots/fixture.sh', str(root)], check=True)
path = root / 'storefront/.taskq/status.json'
state = json.loads(path.read_text())
row = json.loads(pathlib.Path('tests/fixtures/answer-card.json').read_text())['cases'][0]['rows'][0]
question = dict(row, n=41, title='Cart quantity', state='ask')
fallback = dict(n=42, title='Review needs help', state='ask', revision=420, card='Reviewer could not finish. Choose the next step.',
                question=dict(parsed=False, text='Reviewer could not finish. Choose the next step.', choices=[]))
state['tasks'] = [question, fallback, dict(question, n=43, title='Fix login redirect', state='doing'),
                  dict(question, n=44, title='Export invoices', state='later')]
path.write_text(json.dumps(state))
