"""Owner risk decision is separate from a fencing claim; board fixture has no native security effects."""
import copy,json,os
from unittest import mock
from tests.test_single import Base,taskq
from tests import test_repair_reset as prior
class OwnerRisk(Base):
    held=prior.RepairReset.held
    owner=prior.RepairReset.owner
    apply=prior.RepairReset.apply
    def setUp(self):
        super().setUp();self.add('research');self.manager=self.task(1)['pm'];self.held()
        self.git=mock.patch.object(taskq,'update_git',return_value='a'*40);self.git.start();self.addCleanup(self.git.stop)
    def preview(self,**changes):
        args=['repair','--tasks','1','--force','--reason','owner accepts exact historical risk',
              '--accept-legacy-risk','--owner-approval',changes.get('approval','Sentinel_test571'),
              '--retain-result',changes.get('sha','a'*40)]
        return args,json.loads(self.run_cli(*args))
    def test_supported_without_fence_no_false_completion_preserves_audit(self):
        before=copy.deepcopy(self.task(1)['raw']);args,plan=self.preview()
        self.assertEqual(plan['tasks'][0]['status'],'owner-risk-decision-required')
        with self.owner(),mock.patch.object(taskq,'dispatch') as dispatch:self.apply(args,plan)
        dispatch.assert_not_called();item=self.task(1);entry=item['raw']['repair_history'][0]
        self.assertEqual(item['state'],'review');self.assertEqual(item['raw']['result'],before['result'])
        self.assertEqual(entry['snapshot']['raw'],before);self.assertEqual(entry['outcome'],'abandoned-by-owner')
        self.assertFalse(entry['historical_drain_proven']);self.assertFalse(entry['proof']['broad_resource_fenced'])
        self.assertEqual(entry['proof']['denied_resources'],[]);self.assertEqual(entry['proof']['related_leases'],'unchanged-unreconciled')
        self.assertEqual(entry['proof']['unknowns'],list(taskq.REPAIR_ACCEPTED_RISKS))
        self.assertEqual(item['raw']['execution_generation'],1);self.assertEqual(item['raw']['owner_decision'],before['owner_decision'])
        self.assertIsNone(item['raw']['claim']);self.assertIsNone(item['raw']['supervisor'])
        self.assertIsNotNone(entry['snapshot']['raw']['claim']);self.assertIsNotNone(entry['snapshot']['raw']['supervisor'])
        self.assertNotIn('acceptance_receipts',item['raw']);self.assertEqual(item['raw']['scope'],before['scope']);self.assertEqual(item['raw']['deps'],before['deps'])
    def test_approval_exact_result_required_not_just_force(self):
        for extra in ([],['--owner-approval','approved'],['--retain-result','a'*40]):
            with self.assertRaises(SystemExit):self.run_cli('repair','--tasks','1','--force','--reason','r','--accept-legacy-risk',*extra)
        with self.assertRaisesRegex(SystemExit,'exact recorded'):self.preview(sha='b'*40)
    def test_risk_bound_to_one_task_only_no_601612_inherited_scope(self):
        self.add('different task');self.held(2)
        args,_=self.preview();args[3:4]=['1','2']
        with self.assertRaisesRegex(SystemExit,'exactly one'):self.run_cli(*args)
    def test_owner_only_apply(self):
        args,plan=self.preview()
        with self.assertRaisesRegex(SystemExit,'owner shell'):self.apply(args,plan)
        self.assertIn('legacy_recovery',self.task(1)['raw'])
    def test_stale_source_comments_and_result_refuse(self):
        args,plan=self.preview();self.board.comment(1,'new owner decision')
        with self.owner(),self.assertRaisesRegex(SystemExit,'preview changed'):self.apply(args,plan)
        self.assertIn('legacy_recovery',self.task(1)['raw'])
    def test_duplicate_apply_no_second_write_then_changed_approval_refuses(self):
        args,plan=self.preview()
        with self.owner():
            self.apply(args,plan);before=copy.deepcopy(self.board.issues)
            with mock.patch.object(self.board,'update',side_effect=AssertionError('duplicate')):self.assertTrue(self.apply(args,plan)['repeated'])
            changed=args.copy();changed[changed.index('--owner-approval')+1]='Sentinel_other'
            with self.assertRaisesRegex(SystemExit,'conflicting'):self.apply(changed,plan)
        self.assertEqual(before,self.board.issues)
    def test_absent_or_rejected_result_cannot_be_revived(self):
        for result in (False,True):
            self.held(result=result);self.board.issues[1]['labels']=['q-ask','taskq-events']
            with self.assertRaisesRegex(SystemExit,'working review'):self.preview()
    def test_git_commit_evidence_required(self):
        with mock.patch.object(taskq,'update_git',return_value='b'*40),self.assertRaisesRegex(SystemExit,'Git object mismatch'):self.preview()
    def test_known_live_original_root_refuses_not_killed(self):
        path=taskq.CONFIG['root']/'.taskq';path.mkdir(exist_ok=True);(path/'T1.pid').write_text('123 old-worker-1 birth')
        with mock.patch.object(taskq,'read_process',return_value=(123,'old-worker-1','birth')),mock.patch.object(taskq,'process_state',return_value='running'),self.assertRaisesRegex(SystemExit,'known running'):self.preview()
    def test_native_root_changes_after_preview_refuse(self):
        args,plan=self.preview();path=taskq.CONFIG['root']/'.taskq';path.mkdir(exist_ok=True);(path/'T1.pid').write_text('123 old-worker-1 birth')
        with self.owner(),mock.patch.object(taskq,'read_process',return_value=(123,'old-worker-1','birth')),mock.patch.object(taskq,'process_state',return_value='running'),self.assertRaisesRegex(SystemExit,'known running'):self.apply(args,plan)
    def test_false_proof_scope_and_wrong_project_refuse(self):
        _,plan=self.preview();context=plan['tasks'][0]['context'];proof=taskq.repair_risk_proof(context)
        for key,value in [('broad_resource_fenced',True),('historical_drain_proven',True),('denied_resources',list(taskq.REPAIR_RESOURCES)),('unknowns',[])]:
            bad=copy.deepcopy(proof);bad[key]=value
            with self.assertRaisesRegex(ValueError,'risk/checkpoint'):taskq.repair_proof(context,bad,True)
        bad=copy.deepcopy(context);bad['project']['repo']='foreign'
        with self.assertRaisesRegex(SystemExit,'project binding'):taskq.repair_risk_proof(bad)
    def test_original_restorable_snapshot_without_automatic_rollback(self):
        before=copy.deepcopy(self.board.issues[1]);args,plan=self.preview()
        with self.owner():self.apply(args,plan)
        entry=self.task(1)['raw']['repair_history'][0];raw=entry['snapshot']['raw']
        self.assertEqual(raw,taskq.issue_data(before));self.assertEqual(entry['snapshot']['labels'],before['labels'])
        self.assertEqual(entry['snapshot_sha256'],taskq.repair_digest(entry['snapshot']))
