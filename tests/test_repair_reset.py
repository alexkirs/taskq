"""General reset protocol with an authenticated synthetic adapter; zero native security effects."""
import copy
import json
import os
from unittest import mock
from tests.test_single import Base, taskq, SESSION


class FixtureFence:
    """Only attestations produced here authenticate. Not a native drain/fencing implementation."""
    def __init__(self):
        self.attestations = set()
        self.calls = []
        self.blocked = set()
        self.on_fence = None
        self.valid = True

    def attest(self, value):
        self.attestations.add(taskq.repair_digest(value))
        return copy.deepcopy(value)

    def observe(self, context):
        return self.attest({'context': context, 'fence_available': context['task'] not in self.blocked,
                            'checkpoint_verified': True,
                            'stale': context['task'] not in self.blocked, 'unknowns': [],
                            'controllers': 'stopped-or-isolated', 'related_leases': 'released',
                            'denied_resources': list(taskq.REPAIR_RESOURCES), 'historical_drain_proven': True})

    def fence(self, context):
        self.calls.append(context)
        if self.on_fence:
            self.on_fence(context)
        return self.attest({'context': context, 'unknowns': ['historical descendant coverage unrecorded'],
                            'checkpoint_verified': True,
                            'controllers': 'stopped-or-isolated', 'related_leases': 'released',
                            'denied_resources': list(taskq.REPAIR_RESOURCES), 'historical_drain_proven': False,
                            'new_workspace_isolated': True})

    def verify(self, context, value):
        return self.valid and value.get('context') == context and taskq.repair_digest(value) in self.attestations


class RepairReset(Base):
    def setUp(self):
        super().setUp()
        self.fence = self.board.repair_adapter = FixtureFence()
        self.add('technical task')
        self.manager = self.task(1)['pm']
        self.held()

    def held(self, n=1, legacy=True, result=True):
        issue = self.board.issues[n]
        raw = taskq.issue_data(issue)
        raw.update(claim={'runtime': 'codex', 'session': f'old-worker-{n}', 'name': 'mac'},
                   supervisor={'runtime': 'codex', 'session': f'old-supervisor-{n}', 'name': 'mac'},
                   result={'sha': 'a'*40, 'checks': 'preserved research'} if result else None,
                   control_commands={'old-steer': {'request': {'text': 'preserve old instruction'}}},
                   unknown={'retain': [1, 2]}, owner_decision='accept research only; no policy change',
                   model_turns={'worker': {'key': 'old-key', 'generation': 1, 'phase': 'complete'}},
                   model_recovery={'reason': 'missing tool surface'}, application_receipts={'5': {'exact': 'preserve'}})
        if legacy:
            raw['legacy_recovery'] = {'drain_proven': False}
        issue['body'] = taskq.block('original human prose', raw)
        issue['labels'] = ['q-review' if result else 'q-ask', 'taskq-events']
        return copy.deepcopy(raw)

    def owner(self):
        return mock.patch.dict(os.environ, {'TASKQ_RUNTIME': 'claude', 'TASKQ_HOST': 'mac'}, clear=True)

    def preview(self, *numbers, force=True):
        numbers = numbers or (1,)
        args = ['repair', '--tasks', *map(str, numbers)]
        if force:
            args += ['--force', '--reason', 'owner chooses a fresh attempt']
        return args, json.loads(self.run_cli(*args))

    def apply(self, args, plan):
        return json.loads(self.run_cli(*args, '--apply', '--yes', '--expected', plan['digest']))

    def test_force_abandons_unknown_history_preserves_evidence_and_new_identity(self):
        before = copy.deepcopy(self.task(1)['raw'])
        args, plan = self.preview()
        self.assertEqual(self.fence.calls, [])
        with self.owner(), mock.patch.object(taskq, 'dispatch') as dispatch:
            receipt = self.apply(args, plan)
        dispatch.assert_not_called()
        raw = self.task(1)['raw']; entry = raw['repair_history'][0]
        self.assertFalse(receipt['repeated']); self.assertEqual(len(self.fence.calls), 1)
        self.assertEqual(entry['snapshot']['raw'], before)
        self.assertNotIn('control_commands', raw)
        self.assertEqual(raw['repair_checkpoint']['evidence']['owner_decisions']['control_commands'], before['control_commands'])
        self.assertEqual(entry['outcome'], 'abandoned-by-owner')
        self.assertFalse(entry['historical_drain_proven'])
        self.assertEqual(entry['proof']['unknowns'], ['historical descendant coverage unrecorded'])
        self.assertEqual(self.task(1)['state'], 'review')
        self.assertEqual(raw['result'], before['result'])
        self.assertEqual(raw['repair_checkpoint']['step'], 'review-recorded-result')
        for key in ('claim', 'supervisor', 'order'): self.assertIsNone(raw[key])
        self.assertNotIn('legacy_recovery', raw); self.assertNotIn('model_recovery', raw)
        self.assertEqual(raw['unknown'], before['unknown']); self.assertEqual(raw['owner_decision'], before['owner_decision'])
        self.assertEqual(raw['pm'], before['pm']); self.assertEqual(raw['execution_generation'], 1)
        self.assertEqual(taskq.task_branch(self.task(1)), 'taskq-1-g1')
        self.assertEqual(taskq.result_branch(self.task(1)), 'taskq-1')
        self.assertIn('T1-g1 ', taskq.worker_name(self.task(1)))
        self.assertIn('taskq-1-g1', taskq.brief(self.task(1), 'claude'))
        self.assertIn('original human prose', self.board.get(1)['body'])

    def test_same_apply_replay_invokes_no_second_fence_or_board_write(self):
        args, plan = self.preview()
        with self.owner():
            self.apply(args, plan)
            before = copy.deepcopy(self.board.issues)
            with mock.patch.object(self.board, 'update', side_effect=AssertionError('duplicate write')):
                self.assertTrue(self.apply(args, plan)['repeated'])
        self.assertEqual(len(self.fence.calls), 1); self.assertEqual(before, self.board.issues)

    def test_force_without_a_working_result_falls_back_to_approved_specification(self):
        self.held(result=False)
        args, plan = self.preview()
        checkpoint = plan['tasks'][0]['rollback']['checkpoint']
        self.assertEqual(checkpoint['step'], 'resume-approved-specification')
        self.assertTrue(checkpoint['fallback_restart'])
        with self.owner(): self.apply(args, plan)
        self.assertEqual(self.task(1)['state'], 'ready')
        self.assertIsNone(self.task(1)['result'])
        self.assertEqual(self.task(1)['raw']['owner_decision'], 'accept research only; no policy change')

    def test_rejected_result_is_preserved_in_audit_but_not_reaccepted_as_checkpoint(self):
        self.board.issues[1]['labels'] = ['q-ask', 'taskq-events']
        with self.assertRaisesRegex(SystemExit, 'working review checkpoint'):
            self.preview(force=False)
        args, plan = self.preview()
        self.assertEqual(plan['tasks'][0]['target_state'], 'ready')
        with self.owner(): self.apply(args, plan)
        raw = self.task(1)['raw']
        self.assertIsNone(raw['result'])
        self.assertEqual(raw['repair_history'][0]['snapshot']['raw']['result']['sha'], 'a'*40)

    def test_unverified_checkpoint_refuses_without_fence(self):
        original = self.fence.observe
        def unavailable(context):
            value = original(context); value['checkpoint_verified'] = False
            return self.fence.attest(value)
        self.fence.observe = unavailable
        args, plan = self.preview()
        with self.owner(), self.assertRaisesRegex(SystemExit, 'capability unavailable'):
            self.apply(args, plan)
        self.assertEqual(self.fence.calls, [])

    def test_new_result_uses_successor_branch_without_losing_previous_checkpoint(self):
        args, plan = self.preview()
        with self.owner(): self.apply(args, plan)
        current = self.task(1)
        current['result'] = {'sha': 'b'*40}
        self.assertEqual(taskq.result_branch(current), 'taskq-1-g1')
        self.assertEqual(current['raw']['repair_checkpoint']['evidence']['result']['sha'], 'a'*40)

    def test_replay_requires_the_same_reason_and_verified_fence(self):
        args, plan = self.preview()
        with self.owner():
            self.apply(args, plan)
            changed = args[:-1] + ['different abandonment intent']
            with self.assertRaisesRegex(SystemExit, 'conflicting repair replay'): self.apply(changed, plan)
            self.fence.valid = False
            with self.assertRaisesRegex(SystemExit, 'fence verification unavailable'): self.apply(args, plan)
        self.assertEqual(len(self.fence.calls), 1)

    def test_normal_repair_retains_result_and_does_not_abandon_unknown(self):
        args, plan = self.preview(force=False)
        with self.owner(): self.apply(args, plan)
        raw = self.task(1)['raw']
        self.assertEqual(self.task(1)['state'], 'review'); self.assertEqual(raw['result']['sha'], 'a'*40)
        self.assertEqual(raw['repair_history'][0]['outcome'], 'stale-ownership-normalized')
        self.assertNotIn('execution_generation', raw); self.assertEqual(self.fence.calls, [])
        self.assertEqual(raw['repair_history'][0]['snapshot']['raw']['application_receipts'], {'5': {'exact': 'preserve'}})

    def test_force_needs_explicit_tasks_reason_digest_consent_and_owner(self):
        for args in (['repair', '--force'], ['repair', '--tasks', '1', '--force'],
                     ['repair', '--tasks', '1', '--force', '--reason', 'reset', '--apply', '--yes']):
            with self.subTest(args=args), self.assertRaises(SystemExit): self.run_cli(*args)
        args, plan = self.preview()
        with self.assertRaisesRegex(SystemExit, 'owner shell'): self.apply(args, plan)
        self.assertEqual(self.fence.calls, [])

    def test_missing_native_adapter_refuses_before_any_write(self):
        del self.board.repair_adapter
        args, plan = self.preview()
        self.assertEqual(plan['tasks'][0]['status'], 'operator-required')
        self.assertFalse(plan['investigation']['agent_invoked'])
        with self.owner(), mock.patch.object(self.board, 'update', side_effect=AssertionError('unsafe write')):
            with self.assertRaisesRegex(SystemExit, 'capability unavailable'): self.apply(args, plan)
        self.assertIn('legacy_recovery', self.task(1)['raw']); self.assertIsNone(self.board.guard)

    def test_changed_source_refuses_before_fence(self):
        args, plan = self.preview()
        raw = taskq.issue_data(self.board.issues[1]); raw['owner_decision'] = 'newer owner answer'
        self.board.issues[1]['body'] = taskq.block('newer human prose', raw)
        with self.owner(), self.assertRaisesRegex(SystemExit, 'preview changed'): self.apply(args, plan)
        self.assertEqual(self.fence.calls, []); self.assertEqual(self.task(1)['raw']['owner_decision'], 'newer owner answer')

    def test_changed_native_comment_refuses_before_fence(self):
        args, plan = self.preview()
        self.board.comment(1, 'new owner decision in native history')
        with self.owner(), self.assertRaisesRegex(SystemExit, 'preview changed'):
            self.apply(args, plan)
        self.assertEqual(self.fence.calls, [])

    def test_native_comments_are_retained_in_attempt_snapshot(self):
        self.board.comment(1, 'preserve this original owner decision')
        before = copy.deepcopy(self.board.issues[1]['comments'])
        args, plan = self.preview()
        with self.owner(): self.apply(args, plan)
        self.assertEqual(self.task(1)['raw']['repair_history'][0]['snapshot']['comments'], before)

    def test_configured_host_restriction_refuses_before_observation(self):
        self.board.issues[1]['labels'].append('host-win')
        with mock.patch.dict(os.environ, {'TASKQ_HOST_ONLY': 'mac'}), \
                mock.patch.object(self.fence, 'observe', side_effect=AssertionError('outside host')):
            with self.assertRaisesRegex(SystemExit, 'outside TASKQ_HOST_ONLY'):
                self.preview()

    def test_post_fence_changed_source_is_preserved_and_retains_guard(self):
        args, plan = self.preview()
        self.fence.on_fence = lambda _: self.board.issues[1].update(title='concurrently changed')
        with self.owner(), self.assertRaisesRegex(ValueError, 'source changed'): self.apply(args, plan)
        self.assertEqual(self.board.issues[1]['title'], 'concurrently changed')
        self.assertIn('legacy_recovery', self.task(1)['raw']); self.assertIsNotNone(self.board.guard)

    def test_incomplete_resource_fence_refuses_and_no_barrier_is_removed(self):
        args, plan = self.preview(); original = self.fence.fence
        def incomplete(context):
            proof = original(context); proof['denied_resources'].remove('remote-publication'); return self.fence.attest(proof)
        self.fence.fence = incomplete
        with self.owner(), self.assertRaisesRegex(ValueError, 'resource settlement'): self.apply(args, plan)
        self.assertIn('legacy_recovery', self.task(1)['raw']); self.assertIsNotNone(self.board.guard)

    def test_multitask_preflight_checks_all_before_first_fence(self):
        self.add('second technical task'); self.held(2)
        self.fence.blocked.add(2)
        args, plan = self.preview(1, 2)
        with self.owner(), self.assertRaisesRegex(SystemExit, 'capability unavailable'): self.apply(args, plan)
        self.assertEqual(self.fence.calls, [])

    def test_intentional_later_waiting_and_product_holds_are_never_reset(self):
        for labels in (['q-later'], ['q-waiting'], ['q-ask', 'product-hold']):
            self.board.issues[1]['labels'] = labels
            with self.subTest(labels=labels), self.assertRaisesRegex(SystemExit, 'intentional'):
                self.preview()
        self.assertEqual(self.fence.calls, [])

    def test_abandoned_actor_and_reassignment_are_refused(self):
        args, plan = self.preview()
        with self.owner(): self.apply(args, plan)
        current = self.task(1)
        with mock.patch.dict(os.environ, {'CODEX_THREAD_ID': 'old-worker-1', 'TASKQ_RUNTIME': 'codex'}, clear=True):
            self.assertIn('abandoned', taskq.compatibility_reason(current))
        raw = copy.deepcopy(current['raw']); raw['claim'] = {'runtime': 'codex', 'session': 'old-worker-1', 'name': 'mac'}
        with self.assertRaisesRegex(SystemExit, 'abandoned identity'):
            taskq.write_task_verified(self.board.get(1), raw, self.board.get(1)['labels'])
        with self.assertRaisesRegex(SystemExit, 'abandoned identity'):
            taskq.move(current, 'doing', 'spawn', 'invalid old worker', claim=raw['claim'])

    def test_normal_retirement_never_removes_quarantined_attempt_handles(self):
        args, plan = self.preview()
        with self.owner(): self.apply(args, plan)
        current = self.task(1)
        self.assertFalse(taskq.stale({1: current})('codex', 1, 'old-worker-1', False))
        self.board.issues[1]['state'] = 'closed'
        self.assertFalse(taskq.stale({})('codex', 1, 'old-worker-1', False))
        self.assertFalse(taskq.stale({})('codex', 1, 'old-supervisor-1', False))

    def test_generation_names_remain_visible_to_claude_native_role_matching(self):
        raw = self.task(1)['raw']; raw['execution_generation'] = 3
        self.assertTrue(taskq.re.match(taskq.Claude.names, taskq.worker_name(self.task(1))))

    def test_old_worker_cannot_be_selected_as_the_new_manager(self):
        with self.assertRaisesRegex(SystemExit, 'cannot become the reset manager'):
            self.run_cli('repair', '--tasks', '1', '--force', '--reason', 'reset', '--manager', 'codex:old-worker-1:mac')

    def test_explicit_manager_selection_and_fresh_generation_do_not_change_scope_or_deps(self):
        args, plan = self.preview()
        args += ['--manager', 'codex:new-manager:mac']; plan = json.loads(self.run_cli(*args))
        with self.owner(): self.apply(args, plan)
        raw = self.task(1)['raw']; original = raw['repair_history'][0]['snapshot']['raw']
        self.assertEqual(raw['pm'], {'runtime': 'codex', 'session': 'new-manager', 'name': 'mac'})
        self.assertEqual(raw['scope'], original['scope']); self.assertEqual(raw['deps'], original['deps'])
        self.assertTrue(taskq.abandoned_identity(raw, original['pm']))
        self.assertFalse(taskq.abandoned_identity(raw, raw['pm']))

    def test_second_reset_preserves_prior_attempt_and_uses_a_distinct_branch(self):
        args, plan = self.preview()
        with self.owner(): self.apply(args, plan)
        self.held(1, legacy=False, result=False)
        # Use a genuinely new identity; old actor reassignment is not a legal runtime path.
        raw = taskq.issue_data(self.board.issues[1]); raw['claim']['session'] = 'generation-one-worker'
        raw['supervisor']['session'] = 'generation-one-supervisor'
        self.board.issues[1]['body'] = taskq.block('preserved human prose', raw)
        args, plan = self.preview()
        with self.owner(): self.apply(args, plan)
        raw = self.task(1)['raw']; self.assertEqual(len(raw['repair_history']), 2)
        self.assertEqual(raw['execution_generation'], 2); self.assertEqual(taskq.task_branch(self.task(1)), 'taskq-1-g2')
        self.assertEqual(raw['repair_history'][0]['snapshot']['raw']['claim']['session'], 'old-worker-1')
