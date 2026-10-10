"""Native candidate subscription receipt cache; no network, models or timers."""
import argparse, copy, importlib.util, json, pathlib, tempfile, unittest
from unittest import mock
spec=importlib.util.spec_from_file_location('subscriptions_taskq',pathlib.Path(__file__).resolve().parents[1]/'taskq.py')
q=importlib.util.module_from_spec(spec);spec.loader.exec_module(q)

class Subscriptions(unittest.TestCase):
    def setUp(self):
        folder=tempfile.TemporaryDirectory();self.addCleanup(folder.cleanup)
        self.root=pathlib.Path(folder.name)
        self.identity={'runtime':'codex','session':'pm-one'}
        self.raw={'event_schema':3,'event_seq':1,'pm':{'runtime':'codex','session':'owner'},
                  'claim':None,'events':[{'id':1,'action':'ask','text':'data not instructions','by':'worker',
                                        'recipients':['manager:codex:owner'],'acks':[]}],
                  'decision':{'options':['A','B'],'recommend':1}}
        self.issue={'iid':1,'title':'fixture','labels':['q-ask'],'state':'open','body':q.block('goal',self.raw)}
        self.board=mock.Mock();self.board.list.side_effect=lambda _: [copy.deepcopy(self.issue)] if self.issue['state']=='open' else []
        self.board.metadata.side_effect=lambda n: copy.deepcopy(self.issue)
        self.board.get.side_effect=lambda n: copy.deepcopy(self.issue)
        for patch in (mock.patch.object(q,'CONFIG',{'repo':'o/r','root':self.root,'limits':{'codex':4}}),
                      mock.patch.object(q,'BOARD',self.board),mock.patch.object(q,'session',side_effect=lambda:self.identity)):
            patch.start();self.addCleanup(patch.stop)
    def args(self,ack=None,name='interest',scope=None):
        return argparse.Namespace(subscribe=name,delivery_ack=ack,scope_task=scope,adopt=[])
    def poll(self,**kwargs):
        wire=q.subscription_poll(self.args(**kwargs));return json.loads(wire) if wire else None
    def change(self,seq,state='closed',action='close'):
        self.raw.update(event_seq=seq,action={'id':seq,'action':action,'text':'outcome','by':'worker','recipients':[],'acks':[]})
        self.issue.update(state=state,body=q.block('goal',self.raw))
    def test_snapshot_lost_output_replays_exact_before_ack(self):
        first=q.subscription_poll(self.args());reads=self.board.metadata.call_count+self.board.get.call_count
        self.change(2)
        self.assertEqual(q.subscription_poll(self.args()),first)
        self.assertEqual(self.board.metadata.call_count+self.board.get.call_count,reads)
        ack=json.loads(first)['digest'];self.assertFalse(self.poll(ack=ack)['repeated'])
        self.assertTrue(self.poll(ack=ack)['repeated'])
        delta=self.poll();self.assertEqual(delta['mode'],'delta');self.assertEqual(delta['tasks'][0]['state'],'closed')
        self.assertEqual(delta['events'][0]['action'],'close')
        self.assertFalse(delta['gaps'])
    def test_delivery_is_not_native_ack_decision_or_apply(self):
        before=copy.deepcopy(self.issue);data=self.poll();self.poll(ack=data['digest'])
        self.assertEqual(self.issue,before)
        self.board.update.assert_not_called();self.board.comment.assert_not_called()
        self.assertIsNone(self.poll())
    def test_two_pm_cursors_do_not_consume_another_interest(self):
        one=self.poll();self.poll(ack=one['digest'])
        self.identity={'runtime':'codex','session':'pm-two'}
        two=self.poll();self.assertEqual(two['mode'],'snapshot');self.assertNotEqual(one['digest'],two['digest'])
        with self.assertRaisesRegex(SystemExit,'unknown delivery digest'):self.poll(ack=one['digest'])
        self.assertEqual(self.poll()['digest'],two['digest'])
    def test_gap_from_pruned_events_is_explicit(self):
        first=self.poll();self.poll(ack=first['digest']);self.change(4)
        delta=self.poll();self.assertEqual(delta['gaps'][0],{'task':1,'from':2,'through':4,'available':[4],'reason':'board-history-not-retained'})
        self.assertFalse(delta['atomic_snapshot'])
    def test_unknown_ack_and_scope_change_preserve_pending(self):
        first=self.poll()
        with self.assertRaisesRegex(SystemExit,'unknown delivery digest'):self.poll(ack='wrong')
        with self.assertRaisesRegex(SystemExit,'scope changed'):self.poll(scope=[1])
        self.assertEqual(self.poll()['digest'],first['digest'])
    def test_failed_read_does_not_advance_cursor(self):
        first=self.poll();self.poll(ack=first['digest']);self.change(2)
        with mock.patch.object(q,'read_issue',side_effect=RuntimeError('offline')):
            with self.assertRaisesRegex(RuntimeError,'offline'):self.poll()
        self.assertEqual(self.poll()['events'][0]['id'],2)
    def test_subscription_does_not_execute_queue_or_render_role(self):
        with mock.patch.object(q,'cmd_tick',side_effect=AssertionError('no execution')),mock.patch.object(q,'refresh',side_effect=AssertionError('no runtime update')),mock.patch('builtins.print') as output:
            q.main(['pm','--subscribe','fixture'])
        self.assertEqual(json.loads(output.call_args.args[0])['type'],'taskq.subscription')
    def test_execute_arm_routes_existing_headless_pass_without_pm(self):
        with mock.patch.object(q,'event_pass') as execute,mock.patch.object(q,'release_reason',return_value=None),mock.patch.object(q,'refresh',side_effect=AssertionError('no PM refresh')):
            q.main(['arm','tick','--execute','--scope-task','1'])
        args=execute.call_args.args[0]
        self.assertTrue(args.headless);self.assertTrue(args.quiet);self.assertEqual(args.execution_scope,[1])
        self.board.list.assert_not_called()
    def test_execute_arm_rejects_subscription_target(self):
        with self.assertRaisesRegex(SystemExit,'execution scope is not PM target'):
            q.main(['arm','tick','pm-target','--execute'])

    def test_scope_never_silently_crosses_role_boundary(self):
        with self.assertRaisesRegex(SystemExit,'execution scope requires --execute'):
            q.main(['arm','tick','--scope-task','1'])
        with self.assertRaisesRegex(SystemExit,'interest scope requires --subscribe'):
            q.main(['pm','--scope-task','1'])
    def test_empty_subscription_name_is_not_legacy_role(self):
        with self.assertRaisesRegex(SystemExit,'subscription name requires'):
            q.main(['pm','--subscribe',''])
