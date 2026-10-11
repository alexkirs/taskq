"""Shared steering kernel and board adapter invariants; local fixtures only."""
import argparse, contextlib, copy, importlib.util, io, json, os, pathlib, unittest
from unittest import mock
spec=importlib.util.spec_from_file_location('control_taskq', pathlib.Path(__file__).resolve().parents[1]/'taskq.py')
q=importlib.util.module_from_spec(spec);spec.loader.exec_module(q)

class ControlKernel(unittest.TestCase):
    def setUp(self):
        self.actor={'runtime':'codex','session':'pm-full'}
        self.worker={'runtime':'codex','session':'worker-full','name':'host'}
        self.raw={'event_schema':3,'event_seq':1,'pm':self.actor,'claim':self.worker,
                  'supervisor':{'runtime':'codex','session':'boss-full'},'events':[],
                  'model_turns':{'worker':{'phase':'bound','runtime':{'session':'worker-full'}}}}
        self.item={'iid':1,'state':'doing','text':'goal','raw':self.raw}
        self.project={'repo':'o/r','board':'github'}
    def request(self, **changes):
        args=dict(current=self.item,project=self.project,actor=self.actor,
                  revision=q.control_revision(self.item,self.project),command_id='one',text='exact instruction')
        return q.control_request(**{**args,**changes})
    def test_duplicate_replay_conflict_and_semantic_revision(self):
        revision=q.control_revision(self.item,self.project)
        receipt,repeated=self.request();self.assertFalse(repeated)
        before=copy.deepcopy(self.raw)
        self.assertEqual(self.request(),(receipt,False));self.assertEqual(self.raw,before)
        self.raw['control_commands']={'one':receipt}
        self.assertEqual(self.request(revision=revision),(receipt,True))
        for change in ({'text':'changed'},{'revision':'0'*64},{'actor':{'runtime':'owner','session':'shell'}}):
            with self.assertRaisesRegex(ValueError,'conflicting'):
                self.request(**{'revision':revision,**change})
        with self.assertRaisesRegex(ValueError,'stale'):
            self.request(command_id='two',revision=revision)
    def test_authority_uses_full_runtime_and_session(self):
        for actor in (self.actor,self.raw['supervisor'],{'runtime':'owner','session':'shell'}):
            self.assertFalse(self.request(actor=actor)[1])
        for actor in (self.worker,{}, {'runtime':'claude','session':'pm-full'}, {'runtime':'codex','session':'pm-fulx'}):
            with self.assertRaisesRegex(ValueError,'requires recorded'):self.request(actor=actor)
    def test_transport_does_not_change_revision_but_semantics_do(self):
        original=q.control_revision(self.item,self.project)
        for key,value in (('event_seq',9),('events',[{'id':4,'acks':['worker']}]),('model_turns',{}),('action',{'action':'nudge'})):
            changed=copy.deepcopy(self.item);changed['raw'][key]=value
            self.assertEqual(q.control_revision(changed,self.project),original)
        for key,value in (('result',{'sha':'a'*40}),('claim',None),('scope',['new.py']),('deps',[2]),('action_payloads',{'ask':{'text':'new'}})):
            changed=copy.deepcopy(self.item);changed['raw'][key]=value
            self.assertNotEqual(q.control_revision(changed,self.project),original)
    def test_fail_closed_bounds_and_unqualified_paths(self):
        for key,value in (('event_schema',2),('model_recovery',True),('legacy_recovery',{}),('claim',{'runtime':'claude','session':'w'}),('model_turns',{}),('control_commands',{str(n):{} for n in range(32)})):
            raw=copy.deepcopy(self.raw);self.raw[key]=value
            with self.assertRaises(ValueError):self.request()
            self.raw.clear();self.raw.update(raw)
    def test_control_pending_invariant_for_order_delivery_and_application(self):
        target=q.recipient('worker',self.worker)
        for action in ('answer','steer','result'):
            for ack in ([],['supervisor:codex:boss-full'],[target]):
                self.raw['events']=[{'id':2,'action':action,'text':'secret payload','recipients':[target],'acks':ack}]
                self.assertEqual(bool(q.pending_worker_controls(self.raw)),action in ('answer','steer') and target not in ack)


class ControlBoard(unittest.TestCase):
    def setUp(self):
        ControlKernel.setUp(self)
        self.issue={'iid':1,'title':'fixture','labels':['q-doing'],'state':'open','body':q.block('goal',self.raw),'comments':['nudge: malicious comment']}
        self.board=mock.Mock()
        self.board.metadata.side_effect=lambda n:copy.deepcopy(self.issue)
        self.board.get.side_effect=lambda n:copy.deepcopy(self.issue)
        self.board.update.side_effect=lambda n,**values:self.issue.update(values)
        for patch in (mock.patch.object(q,'BOARD',self.board),mock.patch.object(q,'CONFIG',self.project),
                      mock.patch.object(q,'session',return_value=self.actor)):
            patch.start();self.addCleanup(patch.stop)
    def args(self):
        return argparse.Namespace(n=1,revision=q.control_revision(q.parse(self.issue),self.project),command_id='one',text='SECRET instruction')
    def call(self,fn,args):
        with contextlib.redirect_stdout(io.StringIO()) as out:fn(args)
        return json.loads(out.getvalue())
    def test_write_readback_audit_replay_and_inbox_no_ack(self):
        args=self.args();receipt=self.call(q.cmd_steer,args)
        self.assertFalse(receipt['repeated']);self.board.comment.assert_called_once()
        self.assertIn('SECRET instruction',self.board.comment.call_args.args[1])
        before=copy.deepcopy(self.issue)
        replay=self.call(q.cmd_steer,args);self.assertTrue(replay['repeated']);self.assertEqual(before,self.issue)
        inbox=self.call(q.cmd_inbox,argparse.Namespace(n=1,role='worker',history=True))
        self.assertEqual(inbox['pending'][0]['text'],args.text)
        self.assertEqual(inbox['control_commands']['one']['request']['text'],args.text)
        self.assertFalse(inbox['history_complete']);self.assertEqual(before,self.issue)
        self.assertEqual(self.board.update.call_count,1)
    def test_unknown_readback_retains_durable_receipt_for_replay(self):
        args=self.args();original=self.board.metadata.side_effect
        count=0
        def read(n):
            nonlocal count
            count+=1
            if count==3:raise RuntimeError('lost readback')
            return original(n)
        self.board.metadata.side_effect=read
        with self.assertRaisesRegex(RuntimeError,'lost readback'):q.cmd_steer(args)
        self.board.metadata.side_effect=original
        replay=self.call(q.cmd_steer,args)
        self.assertTrue(replay['repeated']);self.assertEqual(self.board.update.call_count,1)
    def test_race_refuses_before_write(self):
        args=self.args();old=copy.deepcopy(self.issue)
        for change in ({'labels':['q-review']},{'title':'changed instructions'},{'assignees':['new-owner']}):
            self.board.metadata.side_effect=[old,{**old,**change}]
            with self.assertRaisesRegex(SystemExit,'changed before'):q.cmd_steer(args)
        self.board.update.assert_not_called()
    def test_pointer_hides_semantics_and_schema3_comments_are_inert(self):
        receipt=self.call(q.cmd_steer,self.args())
        raw=q.issue_data(self.issue)
        for role in ('worker','supervisor'):
            pointer=q.continuation_pointer(1,raw['events'],role)
            self.assertIn('[event 1:2]',pointer);self.assertIn('inbox 1',pointer);self.assertNotIn('SECRET',pointer)
        self.assertNotIn('SECRET',q.event_line(1,{'id':3,'action':'close','text':'SECRET verdict'}))
        self.assertEqual(list(q.legacy_worker_messages(self.issue,self.worker,True)),[])
    def test_late_pending_control_blocks_result_and_publication(self):
        # The admitted turn has no event IDs: this catches events arriving after admission.
        initial=q.parse(self.issue)
        self.call(q.cmd_steer,self.args())
        args=argparse.Namespace(command='result',n=1,sha='a'*40,checks='tests',text='result',option=[],recommend=1,link=[])
        with self.assertRaisesRegex(SystemExit,'unapplied worker control events: 1:2'):q.cmd_move(args)
        with self.assertRaisesRegex(SystemExit,'unapplied worker control events: 1:2'):q.publication_control_check(initial)
        self.assertEqual(q.parse(self.issue)['state'],'doing')
    def test_running_unknown_and_unsupported_workers_preserve_control(self):
        self.call(q.cmd_steer,self.args())
        for state in ('running','unknown','idle'):
            item=q.parse(self.issue);before=copy.deepcopy(self.issue);kind=mock.Mock();kind.state.return_value=state
            with mock.patch.object(q,'model_role_state',return_value=None),mock.patch.object(q,'lead_state',return_value=state),mock.patch.object(q,'age',return_value=1),contextlib.redirect_stderr(io.StringIO()):
                q.follow(item,kind,self.worker,True)
            kind.send.assert_not_called();self.assertEqual(before,self.issue)
    def test_steer_rejects_no_application_adapter_without_write(self):
        raw=q.issue_data(self.issue);raw['claim']['runtime']='claude';self.issue['body']=q.block('goal',raw)
        with self.assertRaisesRegex(SystemExit,'qualified native Codex'):q.cmd_steer(self.args())
        self.board.update.assert_not_called();self.board.comment.assert_not_called()

    def test_closed_inbox_and_cli_read_never_acquire_guard(self):
        self.issue.update(state='closed',labels=[])
        with mock.patch.dict(os.environ,{},clear=True), mock.patch.object(q,'coordination',side_effect=AssertionError('read acquired guard')):
            result=self.call(q.main,['inbox','1','--role','worker','--history'])
        self.assertEqual(result['state'],'closed')
        self.assertEqual(result['task_card']['text'],'goal')
        self.board.update.assert_not_called()
    def test_cli_steer_is_guarded_and_exact_replay_does_not_dispatch(self):
        self.board.list.return_value=[copy.deepcopy(self.issue)]
        args=self.args();argv=['steer','1','--revision',args.revision,'--command-id',args.command_id,'--text',args.text]
        held=[]
        @contextlib.contextmanager
        def guard():
            held.append(True)
            try:yield
            finally:held.pop()
        update=self.board.update.side_effect
        def guarded_update(n,**values):
            self.assertTrue(held);update(n,**values)
        self.board.update.side_effect=guarded_update
        with mock.patch.dict(os.environ,{},clear=True),mock.patch.object(q,'coordination',side_effect=guard),mock.patch.object(q,'release_reason',return_value=None),mock.patch.object(q,'dispatch') as dispatch:
            self.call(q.main,argv);dispatch.assert_called_once_with('steer',[1])
            self.call(q.main,argv);dispatch.assert_called_once()
    def test_answer_authority_matches_exact_runtime_and_full_session(self):
        current=q.parse(self.issue)
        for identity in (self.actor,self.raw['supervisor'],None):
            with mock.patch.object(q,'session',return_value=identity):q.answer_authority(current)
        for identity in (self.worker,{'runtime':'claude','session':self.actor['session']},{'runtime':'codex','session':'other'}):
            with mock.patch.object(q,'session',return_value=identity),self.assertRaisesRegex(SystemExit,'recorded manager'):
                q.answer_authority(current)

    def test_versioned_answer_retains_full_request_after_payload_compaction(self):
        raw=q.issue_data(self.issue)
        raw['decision']={'summary':'choose','options':['one'],'recommend':1}
        self.issue.update(labels=['q-ask'],body=q.block('goal',raw))
        args=argparse.Namespace(n=['1'],text='retained exact answer',command_id='answer-one',
                                question_revision=q.question_revision(q.parse(self.issue)))
        self.board.comment.side_effect=RuntimeError('comment unavailable')
        with contextlib.redirect_stderr(io.StringIO()):self.call(q.answer_command,args)
        raw=q.issue_data(self.issue)
        raw['events']=[]  # fixture after explicit recipients settled and pending history compacted
        raw['action_payloads']['answer']={'id':9,'by':'owner','text':'later answer'}
        self.issue['body']=q.block('goal',raw)
        inbox=self.call(q.cmd_inbox,argparse.Namespace(n=1,role='worker',history=False))
        self.assertEqual(inbox['answer_commands']['answer-one']['request']['text'],'retained exact answer')
        self.assertEqual(inbox['answer_commands']['answer-one']['request']['actor'],self.actor)
    def test_historical_event_lines_keep_legacy_verdict(self):
        event={'id':4,'action':'close','text':'legacy verdict'}
        self.assertIn('legacy verdict',q.event_line(1,event,1))
        self.assertNotIn('legacy verdict',q.event_line(1,event,3))

if __name__=='__main__':unittest.main()
