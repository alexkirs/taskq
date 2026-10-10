# Native Hermes local qualification

On 2026-10-09 the parent executed a fresh authenticated pilot, retained at
`.taskq/hermes-pilot-061c68d12c45/evidence.json`, and verified PASS. The sanitized
[evidence snapshot](hermes-local-qualification.json) records the candidate base HEAD,
dirty state and SHA256 mapping for `AGENTS.md`, `taskq.md` (the contract), `taskq.py`,
`runtimes/hermes.py`, `runtimes/hermes_pilot.py` and `tests/test_single.py`.
All six content hashes matched before this report was added. The disposable fixture
seed SHA is separate from the candidate HEAD; this report is release documentation,
not part of the earlier model-qualified file set.

The actual topology was native Hermes manager → native Hermes supervisor → real
Codex CLI worker → independent supervisor review/close → native manager wake.
Local file-board issue 1 closed. Manager `20261009_163721_e76a30` and supervisor
`20261009_163804_d9ba2c` were distinct; the worker thread was
`00000000-0000-0000-0000-000000000001`. Both worker and supervisor calculated
`17 + 25` in separate successful tool calls, producing `42` with exit code 0.
The supervisor tool result was joined to its exact review turn (persisted user/final
rows 98/112). The exact closed-board outcome woke the manager with a successful
persisted receipt, rows 109/111. Research close performed its genuine fetch and
ancestry gate against the fixture's disposable local bare origin, without a push.

Parent validation:

```text
python3 -m unittest -v tests.test_single
Ran 141 tests in 21.888s
OK (skipped=1)
```

The optional installed-Hermes unittest skipped because its explicit test home/command
were absent. The separately executed authenticated paid topology passed; the skip
is not presented as authenticated test coverage. The parent retained the combined
suite/pilot log separately under the basename `final-suite-pilot-v4.log`.

To reproduce with explicitly pre-provisioned homes and installed gateway Python
(replace placeholders; authenticated execution incurs model usage):

```sh
python3 -m unittest -v tests.test_single
python3 runtimes/hermes_pilot.py --run --timeout 500 \
  --hermes-home '<distinct-named-profile-home>' \
  --codex-home '<explicit-authenticated-Codex-home>' \
  --hermes-command '["<installed-Hermes-Python>","-m","tui_gateway.entry"]'
```

The parent resource envelope was 600 seconds per individual stage, MemoryMax
1280M, MemoryHigh 1024M, MemorySwapMax 256M, CPUQuota 150%, and TasksMax 256;
the pilot used its own 500-second timeout. These are process/resource limits,
not OS network isolation. Provider network is required; instructions and CLI
guards enforce authorized local scope against accidental commands, while absolute
binaries and other network access remain technically available.

Qualification includes explicit local `tick`/`wait` interventions in the harness;
it does not establish autonomous delivery. Shutdown acknowledged native close,
then used SIGKILL for bridge-owned process groups/adopted children. Evidence confirms
both owners and gateways dead; the parent additionally verified the service inactive
with MainPID 0. This is bounded cleanup proof, not exclusively graceful shutdown.

Earlier harness failures were duplicate native titles (fixed with run-unique manager
names and hosts) and a strict calculation parser rejecting the required TaskQ export
prefix (fixed with exact-prefix recognition). They were not provider authentication
failures. Standalone calculation instructions preserve the strict oracle; extra
chained commands remain rejected. Earlier BLOCKED evidence remains unchanged.

This qualifies only the isolated file-board topology and exact candidate contents.
It makes no production-board, publication, restart-durability or hostile-model
containment claim. No default auth/config/core/plugin edits or credential copying
were needed. No Git push or publication is included.
