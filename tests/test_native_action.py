"""Historical schema2 component faults; not current ordinary-board compatibility proof."""
import importlib.util,pathlib,contextlib,io,json
spec=importlib.util.spec_from_file_location('native_fixtures',pathlib.Path(__file__).with_name('test_single.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f);q=f.taskq
class NativeAction(f.Base):
 def setUp(self):
  super().setUp()
  gate=f.mock.patch.object(q,'board_schema_gate');gate.start();self.addCleanup(gate.stop)
  with f.mock.patch.object(q,'dispatch'):self.add()
  raw=q.issue_data(self.board.get(1));raw.update(event_schema=2,claim={'runtime':'claude','session':f.SESSION,'name':'mac'},event_seq=1,
   events=[{'id':1,'action':'answer','text':'accepted artifact\n','by':None,'recipients':[f'worker:claude:{f.SESSION}'],'acks':[]}])
  self.board.issues[1].update(body=q.block('native qualification',raw),labels=['q-doing'])
 def apply(self,path='artifact.txt'):return json.loads(self.run_cli('apply-event','1:1','--artifact',path))
 def test_authenticated_assignee_and_host_restrictions_before_effect(self):
  self.board.issues[1]['labels'].append('assignee-only')
  self.board.issues[1]['assignees']=['assigned-user']
  with f.mock.patch.object(self.board,'user',return_value='other-user',create=True),self.assertRaises(SystemExit):self.apply()
  self.board.issues[1]['labels'].remove('assignee-only')
  with f.mock.patch.dict('os.environ',{'TASKQ_HOST_ONLY':'other-host'}),self.assertRaises(SystemExit):self.apply()
  self.assertFalse((self.root/'artifact.txt').exists());self.assertIsNone(self.board.guard)
 def test_action_receipt_ack_and_duplicate_no_rewrite(self):
  result=self.apply();path=self.root/'artifact.txt';before=path.stat().st_mtime_ns
  self.assertEqual(path.read_text(),'accepted artifact\n');self.assertEqual(result['phase'],'complete')
  self.assertEqual(self.apply(),result);self.assertEqual(path.stat().st_mtime_ns,before)
  raw=self.task(1)['raw'];self.assertEqual(raw['events'][0]['acks'],[f'worker:claude:{f.SESSION}'])
  self.assertEqual(len(raw['operations']),1);self.assertIsNone(self.board.guard)
 def test_crash_after_artifact_before_board_receipt_preserves_recovery(self):
  original=self.board.update;count=0
  def broken(*args,**kwargs):
   nonlocal count
   count+=1
   if count==2:raise RuntimeError('receipt response unavailable')
   original(*args,**kwargs)
  with f.mock.patch.object(self.board,'update',side_effect=broken),self.assertRaises(RuntimeError):self.apply()
  self.assertTrue((self.root/'artifact.txt').exists());self.assertIsNotNone(self.board.guard)
  self.assertEqual(self.task(1)['raw']['events'][0]['acks'],[])
  self.board.release(self.board.guard) # exact fixture grant after this controller has unwound; no production recovery
  before=(self.root/'artifact.txt').stat().st_mtime_ns;self.assertEqual(self.apply()['phase'],'complete')
  self.assertEqual((self.root/'artifact.txt').stat().st_mtime_ns,before)
 def test_lost_final_response_readback_is_idempotent(self):
  original=self.board.update;count=0
  def lost(*args,**kwargs):
   nonlocal count
   count+=1;original(*args,**kwargs)
   if count==2:raise RuntimeError('final response lost')
  with f.mock.patch.object(self.board,'update',side_effect=lost),self.assertRaises(RuntimeError):self.apply()
  self.board.release(self.board.guard)
  before=json.dumps(self.board.issues,sort_keys=True);self.apply();self.assertEqual(before,json.dumps(self.board.issues,sort_keys=True))
 def test_foreign_actor_legacy_ack_and_path_escape_refused(self):
  with f.mock.patch.object(q,'session',return_value={'runtime':'claude','session':'foreign'}),self.assertRaises(SystemExit):self.apply()
  with self.assertRaises(SystemExit):self.run_cli('ack','1:1')
  with self.assertRaises(SystemExit):self.apply('../escape.txt')
  self.assertFalse((self.root/'artifact.txt').exists());self.assertIsNone(self.board.guard)
 def test_existing_different_file_is_never_overwritten(self):
  path=self.root/'artifact.txt';path.write_text('preserve owner file')
  with self.assertRaises(ValueError):self.apply()
  self.assertEqual(path.read_text(),'preserve owner file');self.assertEqual(self.task(1)['raw']['events'][0]['acks'],[])
 def test_changed_path_or_artifact_receipt_cannot_replay(self):
  self.apply()
  with self.assertRaises(SystemExit):self.apply('other.txt')
  (self.root/'artifact.txt').write_text('tampered')
  with self.assertRaises(ValueError):self.apply()
  self.assertFalse((self.root/'other.txt').exists())

 def test_parked_task_refuses_effect_and_completed_replay(self):
  self.board.issues[1]['labels']=['q-later']
  with self.assertRaises(SystemExit):self.apply()
  self.assertFalse((self.root/'artifact.txt').exists())
  self.board.issues[1]['labels']=['q-doing'];self.apply()
  before=json.dumps(self.board.issues,sort_keys=True)
  self.board.issues[1]['labels']=['q-later']
  with self.assertRaises(SystemExit):self.apply()
  self.assertEqual(self.task(1)['raw']['events'][0]['acks'],[f'worker:claude:{f.SESSION}'])

 def test_manager_question_native_ack_and_sender_refusal(self):
  raw=self.task(1)['raw'];raw['pm']={'runtime':'claude','session':f.SESSION,'name':'mac'}
  raw['events'][0].update(action='ask',recipients=[f'manager:claude:{f.SESSION}'])
  self.board.issues[1].update(body=q.block('question',raw),labels=['q-ask'])
  def handle():return json.loads(self.run_cli('apply-event','1:1','--role','manager','--artifact','question.txt'))
  result=handle();before=(self.root/'question.txt').stat().st_mtime_ns
  self.assertEqual(result['phase'],'complete');self.assertFalse(result['task_accepted'])
  self.assertEqual(self.task(1)['raw']['events'][0]['acks'],[f'manager:claude:{f.SESSION}'])
  self.assertEqual(handle(),result);self.assertEqual((self.root/'question.txt').stat().st_mtime_ns,before)
  with self.assertRaises(SystemExit):self.run_cli('ack','1:1','--pm',f.SESSION)
  with f.mock.patch.object(q,'session',return_value={'runtime':'claude','session':'foreign'}),self.assertRaises(SystemExit):handle()
  (self.root/'question.txt').write_text('tampered')
  with self.assertRaises(ValueError):handle()

 def migration_fixture(self,claim=None):
  raw=self.task(1)['raw'];raw.update(event_schema=1,claim=claim,pm=q.origin(),unknown={'preserve':[1,2]},history=['keep history'])
  raw['events'][0].update(action='ask',recipients=[f'manager:claude:{f.SESSION}'])
  self.board.issues[1].update(body='  human preamble\n'+q.block('',raw)+'\n trailing human text ',labels=['q-ask'])
  return raw
 def test_native_migration_does_not_strand_unclaimed_model_tasks(self):
  raw=self.migration_fixture();original=self.board.get(1)
  for argv in [('migrate','--native-receipts'),('migrate','--native-receipts','--apply','--controllers-stopped')]:
   with self.subTest(argv=argv),self.assertRaisesRegex(SystemExit,'model execution adapter unqualified'):
    self.run_cli(*argv)
   self.assertEqual(self.board.get(1),original);self.assertIsNone(self.board.guard)
  self.assertEqual(self.task(1)['raw'],raw)
 def test_native_migration_unknown_legacy_handle_refuses_before_writes(self):
  claim={'runtime':'codex','session':'legacy-worker','name':'mac'};self.migration_fixture(claim)
  directory=self.root/'.taskq';directory.mkdir(exist_ok=True);handle=directory/'T1.pid';handle.write_text('999999 legacy-worker')
  before=self.board.get(1);pending=handle.read_bytes()
  with self.assertRaisesRegex(SystemExit,'unknown/running'):
   self.run_cli('migrate','--native-receipts','--apply','--controllers-stopped')
  self.assertEqual(self.board.get(1),before);self.assertEqual(handle.read_bytes(),pending);self.assertIsNone(self.board.guard)
 def test_native_migration_foreign_pm_requires_legitimate_invoker(self):
  self.migration_fixture();before=self.board.get(1)
  with f.mock.patch.object(q,'origin',return_value={'runtime':'claude','session':'foreign','name':'mac'}),self.assertRaisesRegex(SystemExit,'must invoke'):
   self.run_cli('migrate','--native-receipts','--apply','--controllers-stopped')
  self.assertEqual(self.board.get(1),before)

 def test_native_migration_dead_cli_does_not_prove_app_drain(self):
  import subprocess,sys
  claim={'runtime':'codex','session':'owned-completed-worker','name':'mac'};raw=self.migration_fixture(claim)
  child=f.REAL_POPEN([sys.executable,'-c','import time;time.sleep(0.2)'])
  birth=q.process_identity(child.pid)[1];self.assertTrue(birth);child.wait(timeout=3)
  directory=self.root/'.taskq';directory.mkdir(exist_ok=True);handle=directory/'T1.pid'
  handle.write_text(f'{child.pid} {claim["session"]} {birth}');before=handle.read_bytes()
  original=self.board.get(1)
  for observation in ({'state':'unknown','code':'active-writer','problems':[]},
                      {'state':'unknown','problems':[]},{'state':'idle','problems':[]}):
   for argv in [('migrate','--native-receipts'),('migrate','--native-receipts','--apply','--controllers-stopped')]:
    with self.subTest(observation=observation,argv=argv),f.mock.patch.object(q.Codex,'observe',return_value=observation),self.assertRaisesRegex(SystemExit,'app ownership/drain'):
     self.run_cli(*argv)
    self.assertEqual(self.board.get(1),original);self.assertEqual(handle.read_bytes(),before);self.assertIsNone(self.board.guard)
 def test_native_migration_preflights_entire_board_before_first_write(self):
  self.migration_fixture();other=self.board.get(1);other['iid']=2
  raw=q.issue_data(other);raw['pm']={**raw['pm'],'name':'other-host'};other['body']=q.block('foreign',raw)
  self.board.issues[2]=other;before=json.dumps(self.board.issues,sort_keys=True)
  with f.mock.patch.object(self.board,'update',wraps=self.board.update) as writes,self.assertRaisesRegex(SystemExit,'model execution adapter unqualified'):
   self.run_cli('migrate','--native-receipts','--apply','--controllers-stopped')
  writes.assert_not_called();self.assertEqual(json.dumps(self.board.issues,sort_keys=True),before)
