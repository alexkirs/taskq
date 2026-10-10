"""Actual controlled children + atomic SQLite providers on disposable boards only."""
import contextlib,hashlib,importlib.util,io,json,pathlib,tempfile,time,unittest
from unittest import mock
spec=importlib.util.spec_from_file_location('lifecycle_fixtures',pathlib.Path(__file__).with_name('test_single.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f);q=f.taskq

class Lifecycle(unittest.TestCase):
 def setUp(self):
  self.directory=tempfile.TemporaryDirectory();self.addCleanup(self.directory.cleanup);self.root=pathlib.Path(self.directory.name).resolve()
  self.original=(q.CONFIG,q.BOARD);self.addCleanup(self.restore)
  q.CONFIG={'board':'fixture.py','repo':'fixture/project','root':self.root,'hosts':{},'capacity':{
   'project_caps':{'slots':1},'host_caps':{'slots':2,'heavy':1},'host_path':str(self.root/'host.sqlite')}}
  self.actor={'runtime':'codex','session':'fixture-supervisor','name':q.machine()}
  self.patch=mock.patch.object(q,'origin',return_value=self.actor);self.patch.start();self.addCleanup(self.patch.stop)
  # Exercise retained historical component fault paths, never ordinary schema2 admission.
  gate=mock.patch.object(q,'board_schema_gate');gate.start();self.addCleanup(gate.stop)
  q.BOARD=self.board=f.FakeBoard();self.project=q.SQLiteCapacity(self.root/'project.sqlite','project','fixture/project',{'slots':1})
  self.board.capacity_provider=lambda caps:self.project
  raw={'event_schema':2,'event_seq':1,'events':[{'id':1,'action':'answer','text':'first answer\n','by':self.actor,
   'recipients':['worker:controlled-artifact:fixture-worker'],'acks':[]}],
   'claim':{'runtime':'controlled-artifact','session':'fixture-worker','name':q.machine()},'supervisor':self.actor,'pm':self.actor,
   'scope':['artifact-{event}.txt'],'order':{'text':'preserve order'},'history':[{'kind':'original'}],
   'admission_age':123,'execution':{'kind':'controlled-artifact','event':1,'artifact':'artifact-{event}.txt','demand':{'slots':1,'heavy':1}},
   'acceptance_criteria':{'kind':'answer-artifact','event':1,'artifact':'artifact-1.txt','sha256':hashlib.sha256(b'first answer\n').hexdigest()}}
  self.board.issues[1]={'iid':1,'title':'Disposable finite artifact task','labels':['q-ready'],'state':'open','body':q.block('fixture',raw),
   'comments':[],'updated_at':'2026-10-10T00:00:00Z'}
  self.addCleanup(self.drain_children)
 def restore(self):q.CONFIG,q.BOARD=self.original
 def raw(self):return q.issue_data(self.board.get(1))
 def test_eligibility_refused_before_capacity_initialization(self):
  self.board.issues[1]['labels'].append('assignee-only');self.board.issues[1]['assignees']=['assigned-user']
  with mock.patch.object(self.board,'user',return_value='other-user',create=True),self.assertRaises(SystemExit):self.invoke('admit')
  self.board.issues[1]['labels'].remove('assignee-only')
  with mock.patch.dict('os.environ',{'TASKQ_HOST_ONLY':'other-host'}),self.assertRaises(SystemExit):self.invoke('admit')
  q.CONFIG['assignee']='different-user'
  with self.assertRaises(SystemExit):self.invoke('admit')
  self.assertFalse((self.root/'host.sqlite').exists());self.assertIsNone(self.board.guard)
  self.assertNotIn('lifecycle',self.raw())
 def host(self):return q.SQLiteCapacity(self.root/'host.sqlite','host',self.actor['name'],{'slots':2,'heavy':1})
 def invoke(self,action,text=None):
  output=io.StringIO()
  with contextlib.redirect_stdout(output):q.main(['lifecycle','1',action]+(['--text',text] if text is not None else []))
  return json.loads(output.getvalue())
 def settle_fixture_guard(self):
  if self.board.guard:self.board.release(self.board.guard) # only own unwound test controller, exact fixture token
 def drain_children(self):
  self.settle_fixture_guard()
  if (self.root/'host.sqlite').exists():
   host=self.host()
   with host.transaction() as db:rows=[(k,json.loads(v)) for k,v in db.execute('SELECT id,body FROM capacity_lease')]
   for key,value in rows:
    if value['phase'] in ('bound','drained','launching','reserved','waiting'):
     runtime=q.ControlledArtifactRuntime(host,self.root);receipt=runtime.drain(key,value['request'])
     if receipt:host.release(key)
 def wait_file(self,path='artifact-1.txt'):
  deadline=time.monotonic()+3
  while not (self.root/path).exists() and time.monotonic()<deadline:time.sleep(.01)
  self.assertTrue((self.root/path).exists())
 def test_real_child_park_answer_resume_and_repeat(self):
  original={k:self.raw()[k] for k in ('claim','supervisor','pm','history','order')}
  start=self.invoke('admit');self.assertEqual(start['phase'],'active');self.assertFalse(start['task_accepted']);self.wait_file()
  key=self.raw()['lifecycle']['key'];child=start['child'];self.assertEqual(q.process_state(child['pid'],child['birth']),'running')
  with self.assertRaises(SystemExit):self.invoke('accept') # file/exit/active child is not settled acceptance
  self.assertEqual(self.invoke('park')['phase'],'parked')
  self.assertEqual(q.process_state(child['pid'],child['birth']),'dead');self.assertEqual(self.host().observe(key)['phase'],'released')
  self.assertEqual(self.project.observe(key)['phase'],'released');self.assertTrue(self.invoke('accept')['task_accepted'])
  self.invoke('answer','second answer\n');self.assertEqual(self.raw()['events'][1]['acks'],[])
  resumed=self.invoke('resume');self.assertEqual(resumed['generation'],2);self.wait_file('artifact-2.txt')
  self.assertEqual(self.invoke('resume')['child'],resumed['child']);self.assertEqual(self.raw()['lifecycle']['age'],123)
  self.assertEqual(self.raw()['claim'],original['claim']);self.assertEqual((self.root/'artifact-2.txt').read_text(),'second answer\n')
  self.invoke('park')
  for k,v in original.items():self.assertEqual(self.raw()[k],v)
  with self.assertRaises(SystemExit):self.invoke('accept') # unchanged old task criteria cannot accept a new answer
  raw=self.raw();raw['acceptance_criteria']={'kind':'answer-artifact','event':2,'artifact':'artifact-2.txt','sha256':hashlib.sha256(b'second answer\n').hexdigest()}
  self.board.issues[1]['body']=q.block('fixture',raw)
  self.assertTrue(self.invoke('accept')['task_accepted']);self.assertEqual(len(self.raw()['acceptance_receipts']),2)
  self.assertIsNone(self.raw().get('result'));self.assertEqual(self.board.issues[1]['state'],'open')
 def test_crash_at_each_park_board_boundary_reconciles_without_stranding(self):
  self.invoke('admit');self.wait_file();key=self.raw()['lifecycle']['key'];update=self.board.update
  for failure in (1,2,3):
   if self.raw()['lifecycle']['phase']=='parked':self.invoke('resume');key=self.raw()['lifecycle']['key'];self.wait_file()
   count=0
   def crash(*args,**kwargs):
    nonlocal count
    count+=1
    if count==failure:raise RuntimeError('injected controller crash')
    return update(*args,**kwargs)
   with mock.patch.object(self.board,'update',side_effect=crash),self.assertRaises(RuntimeError):self.invoke('park')
   self.settle_fixture_guard();self.assertEqual(self.invoke('park')['phase'],'parked')
   self.assertEqual(self.host().observe(key)['phase'],'released');self.assertEqual(self.project.observe(key)['phase'],'released')
 def test_lost_host_release_and_resume_response_reconciled(self):
  self.invoke('admit');self.wait_file();release=q.SQLiteCapacity.release
  def lost(provider,key):
   value=release(provider,key)
   if provider.scope=='host':raise RuntimeError('lost authoritative release response')
   return value
  with mock.patch.object(q.SQLiteCapacity,'release',lost),self.assertRaises(RuntimeError):self.invoke('park')
  key=self.raw()['lifecycle']['key'];self.assertEqual(self.host().observe(key)['phase'],'released')
  self.settle_fixture_guard();self.invoke('reconcile');self.assertEqual(self.project.observe(key)['phase'],'released')
  start=q.ControlledArtifactRuntime.start
  def lost_start(*args):start(*args);raise RuntimeError('lost native start response')
  with mock.patch.object(q.ControlledArtifactRuntime,'start',lost_start),self.assertRaises(RuntimeError):self.invoke('resume')
  child=self.host().observe(self.raw()['lifecycle']['key'])['child'];self.settle_fixture_guard()
  self.assertEqual(self.invoke('resume')['child'],child);self.invoke('park')
 def test_unknown_drain_never_releases_and_answer_preserved(self):
  self.invoke('admit');self.wait_file();key=self.raw()['lifecycle']['key']
  with mock.patch.object(q.ControlledArtifactRuntime,'drain',return_value=None):
   self.assertEqual(self.invoke('park')['phase'],'draining')
  self.assertEqual(self.host().observe(key)['phase'],'bound');self.assertEqual(self.project.observe(key)['phase'],'reserved')
  self.invoke('answer','pending during drain')
  with self.assertRaises(SystemExit):self.invoke('resume')
  self.invoke('park');self.assertEqual(self.raw()['events'][1]['text'],'pending during drain')
 def test_unbound_launch_crash_revoked_then_same_session_resume(self):
  with mock.patch.object(q.ControlledArtifactRuntime,'start',side_effect=RuntimeError('before native spawn')),self.assertRaises(RuntimeError):self.invoke('admit')
  self.settle_fixture_guard();key=self.raw()['lifecycle']['key'];self.host().launch(key)
  self.assertEqual(self.invoke('park')['phase'],'parked')
  with self.assertRaises(ValueError):self.host().bind(key) # delayed predecessor can no longer act
  self.assertEqual(self.invoke('resume')['generation'],2);self.invoke('park')
 def test_runtime_and_backend_capability_refuse_before_effect(self):
  del self.board.capacity_provider
  before=self.board.get(1)
  with self.assertRaises(SystemExit):self.invoke('admit')
  self.assertEqual(self.board.get(1),before);self.assertFalse((self.root/'host.sqlite').exists());self.assertEqual(self.board.guard_serial,0)
 def test_task_acceptance_wrong_authority_artifact_and_criteria(self):
  self.invoke('admit');self.wait_file();self.invoke('park')
  with mock.patch.object(q,'origin',return_value={**self.actor,'session':'foreign'}),self.assertRaises(SystemExit):self.invoke('accept')
  (self.root/'artifact-1.txt').write_text('tampered')
  with self.assertRaises(SystemExit):self.invoke('accept')
  self.assertNotIn('acceptance_receipts',self.raw())

 def test_native_child_crash_without_receipt_releases_but_never_accepts(self):
  self.invoke('admit');self.wait_file();key=self.raw()['lifecycle']['key'];host=self.host()
  process=q.ControlledArtifactRuntime.children[(str(host.path),key)]
  # Actual bounded child loses provider access, fails its read and its final receipt write.
  # Only this disposable host DB is locked; no process signal or production host is touched.
  with host.transaction():
   self.assertNotEqual(process.wait(timeout=12),0)
  self.assertEqual(host.observe(key)['phase'],'bound')
  input_path=self.root/'.taskq'/'controlled'/(key+'.json');original_input=input_path.read_bytes()
  input_path.write_bytes(original_input+b' ')
  with self.assertRaises(ValueError):self.invoke('park')
  self.assertEqual(host.observe(key)['phase'],'bound');self.assertEqual(self.project.observe(key)['phase'],'reserved')
  input_path.write_bytes(original_input);self.settle_fixture_guard()
  self.assertEqual(self.invoke('park')['phase'],'parked')
  self.assertEqual(host.observe(key)['drain']['application']['status'],'unknown')
  self.assertEqual(self.raw()['events'][0]['acks'],[])
  with self.assertRaises(SystemExit):self.invoke('accept')
  self.assertEqual(self.project.observe(key)['phase'],'released')
 def test_lost_reservation_responses_are_reconciled_without_repeating_spawn(self):
  reserve=q.SQLiteCapacity.reserve
  for scope in ('project','host'):
   def lost(provider,key,request):
    value=reserve(provider,key,request)
    if provider.scope==scope:raise RuntimeError('lost reservation response')
    return value
   with mock.patch.object(q.SQLiteCapacity,'reserve',lost),self.assertRaises(RuntimeError):self.invoke('resume' if self.raw().get('lifecycle') else 'admit')
   self.settle_fixture_guard();self.assertEqual(self.invoke('reconcile')['phase'],'active');self.wait_file();self.invoke('park')
 def test_changed_supervisor_and_unknown_birth_cannot_resume_or_release(self):
  self.invoke('admit');self.wait_file();self.invoke('park');raw=self.raw();raw['supervisor']={**self.actor,'session':'replacement'}
  self.board.issues[1]['body']=q.block('fixture',raw)
  with mock.patch.object(q,'origin',return_value=raw['supervisor']),self.assertRaises(SystemExit):self.invoke('resume')
  self.assertEqual(self.raw()['lifecycle']['generation'],1)

 def test_unknown_birth_refuses_dead_child_reconciliation(self):
  self.invoke('admit');self.wait_file();key=self.raw()['lifecycle']['key'];host=self.host()
  # A bound dead-child reconciliation must match the actual admitted code/input; no manual boolean proof.
  with host.transaction() as db:
   value=host.row(db,key);original_child=dict(value['child']);value['child']['birth']=None;host.put(db,key,value)
  with self.assertRaises(ValueError):host.reconcile_dead(key,host.observe(key)['runtime'])
  # Restore only the original fixture binding retained before intentional tampering.
  self.assertEqual(q.process_state(original_child['pid'],original_child['birth']),'running')
  with host.transaction() as db:
   value=host.row(db,key);value['child']=original_child;host.put(db,key,value)
  self.invoke('park')

 def command(self,*args):
  output=io.StringIO()
  # Historical schema2 component fixtures, not current ordinary-board compatibility proof.
  # The current-board gate is tested separately; preserve these fault tests for the reducer.
  with contextlib.redirect_stdout(output),mock.patch.object(q,'dispatch'),mock.patch.object(q,'refresh'),mock.patch.object(q,'retire'),mock.patch.object(q,'board_schema_gate'):
   q.main(list(args))
  return output.getvalue()
 def tick(self):
  self.command('tick','--quiet')
 def second_task(self):
  import copy
  issue=copy.deepcopy(self.board.issues[1]);raw=q.issue_data(issue)
  raw['supervisor']=dict(self.actor);raw['pm']=dict(self.actor)
  raw['claim']['session']='second-worker';raw['events'][0]['recipients']=['worker:controlled-artifact:second-worker']
  raw['execution']['artifact']='second-{event}.txt';raw.pop('lifecycle',None)
  issue.update(iid=2,title='Independent task',body=q.block('second',raw),labels=['q-ready'])
  self.board.issues[2]=issue
 def test_normal_run_tick_ack_result_not_demo_route(self):
  original={k:self.raw()[k] for k in ('claim','supervisor','pm','history','order')}
  with mock.patch.object(q,'cmd_lifecycle',side_effect=AssertionError('demonstration route called')):
   start=json.loads(self.command('run','1'));self.assertEqual(start['phase'],'active')
   self.tick();raw=self.raw();self.assertEqual(raw['lifecycle']['phase'],'parked')
   self.assertEqual(raw['events'][0]['acks'],['worker:controlled-artifact:fixture-worker'])
   self.command('ack','1:1');before=json.dumps(self.board.issues,sort_keys=True)
   self.command('ack','1:1');self.assertEqual(before,json.dumps(self.board.issues,sort_keys=True))
   result=json.loads(self.command('result','1','--text','Bounded artifact checked'))
   self.assertTrue(result['task_accepted']);self.assertEqual(self.raw()['result']['kind'],'answer-artifact')
   self.assertEqual(q.parse(self.board.get(1))['state'],'review')
   before=json.dumps(self.board.issues,sort_keys=True)
   self.command('result','1');self.tick();self.assertEqual(before,json.dumps(self.board.issues,sort_keys=True))
  for k,v in original.items():self.assertEqual(self.raw()[k],v)
 def test_normal_tick_automatically_admits_ready_and_never_repeats_park(self):
  self.tick();self.assertEqual(self.raw()['lifecycle']['phase'],'parked')
  self.assertEqual((self.root/'artifact-1.txt').read_text(),'first answer\n')
  before=json.dumps(self.board.issues,sort_keys=True);self.tick()
  self.assertEqual(before,json.dumps(self.board.issues,sort_keys=True))
 def test_accepted_result_refuses_direct_and_normal_rework_before_effects(self):
  self.tick();self.command('result','1')
  before=json.dumps(self.board.issues,sort_keys=True)
  with mock.patch.object(q.ControlledArtifactRuntime,'start',side_effect=AssertionError('accepted work restarted')):
   for argv in [('lifecycle','1','admit'),('lifecycle','1','resume'),
                ('lifecycle','1','answer','--text','new answer'),('run','1'),('answer','1','--text','new answer')]:
    with self.subTest(argv=argv),self.assertRaisesRegex(SystemExit,'rework'):
     self.command(*argv)
    self.assertEqual(before,json.dumps(self.board.issues,sort_keys=True));self.assertIsNone(self.board.guard)
   self.tick();self.assertEqual(before,json.dumps(self.board.issues,sort_keys=True))
  self.assertEqual(self.invoke('park')['phase'],'parked');self.command('ack','1:1')
 def test_result_reconcile_cannot_enter_admission(self):
  self.tick();self.command('result','1')
  original=self.raw()
  for phase in (None,'waiting','reserving','starting','active'):
   raw=json.loads(json.dumps(original))
   if phase is None:raw.pop('lifecycle',None)
   else:raw['lifecycle']['phase']=phase
   self.board.issues[1]['body']=q.block('fixture',raw)
   before=json.dumps(self.board.issues,sort_keys=True)
   with mock.patch.object(q.ControlledArtifactRuntime,'start',side_effect=AssertionError('result restarted')):
    with self.assertRaisesRegex(SystemExit,'rework'):self.command('lifecycle','1','reconcile')
   self.assertEqual(before,json.dumps(self.board.issues,sort_keys=True));self.assertIsNone(self.board.guard)
 def test_normal_answer_and_same_session_resume_requires_new_acceptance(self):
  self.tick();self.command('answer','1','--text','second answer\n')
  self.assertEqual(q.parse(self.board.get(1))['state'],'ready')
  self.assertEqual(self.raw()['events'][1]['acks'],[])
  self.tick();self.assertEqual(self.raw()['lifecycle']['generation'],2)
  self.assertEqual((self.root/'artifact-2.txt').read_text(),'second answer\n')
  with self.assertRaises(SystemExit):self.command('result','1')
  raw=self.raw();raw['acceptance_criteria']={'kind':'answer-artifact','event':2,'artifact':'artifact-2.txt',
   'sha256':hashlib.sha256(b'second answer\n').hexdigest()}
  self.board.issues[1]['body']=q.block('fixture',raw)
  self.assertTrue(json.loads(self.command('result','1'))['task_accepted'])
 def test_unknown_drain_keeps_slot_and_pending_answer_until_future_pass(self):
  self.command('run','1');self.wait_file();key=self.raw()['lifecycle']['key']
  with mock.patch.object(q.ControlledArtifactRuntime,'drain',return_value=None):
   self.command('later','1');self.command('answer','1','--text','retained')
  self.assertEqual(self.raw()['lifecycle']['phase'],'draining')
  self.assertEqual(self.host().observe(key)['phase'],'bound');self.assertEqual(self.project.observe(key)['phase'],'reserved')
  self.assertEqual(self.raw()['events'][1]['acks'],[])
  self.tick();self.assertEqual(q.parse(self.board.get(1))['state'],'ready')
  self.tick();self.assertEqual(self.raw()['lifecycle']['generation'],2)
  self.assertEqual((self.root/'artifact-2.txt').read_text(),'retained')
 def test_unsupported_or_foreign_task_does_not_block_independent_ready_task(self):
  self.second_task();raw=self.raw();raw['claim']['runtime']='codex';raw['execution']['kind']='codex'
  self.board.issues[1]['body']=q.block('fixture',raw);before=self.board.get(1)
  self.tick();self.assertEqual(self.board.get(1),before)
  self.assertEqual(q.issue_data(self.board.get(2))['lifecycle']['phase'],'parked')
 def test_foreign_controller_keeps_exact_task_and_grants(self):
  self.command('run','1');self.wait_file();self.second_task();raw=self.raw()
  raw['supervisor']={**self.actor,'session':'other-supervisor'};self.board.issues[1]['body']=q.block('fixture',raw)
  before=self.board.get(1);key=raw['lifecycle']['key']
  self.tick();self.assertEqual(self.board.get(1),before);self.assertEqual(self.host().observe(key)['phase'],'bound')
  # second waits for the independently occupied heavy grant, never steals it
  self.assertEqual(q.issue_data(self.board.get(2))['lifecycle']['phase'],'waiting')
 def test_final_ack_stale_readback_retains_guard_and_reentry_verifies(self):
  self.command('run','1');self.wait_file();update=self.board.update;get=self.board.get;count=0;stale=None
  def write(*args,**kwargs):
   nonlocal count,stale
   count+=1
   if count==3:stale=get(1)
   update(*args,**kwargs)
  def read(n):
   nonlocal stale
   if stale is not None:
    value=stale;stale=None;return value
   return get(n)
  with mock.patch.object(self.board,'update',side_effect=write),mock.patch.object(self.board,'get',side_effect=read),self.assertRaises(RuntimeError):
   self.command('later','1')
  self.assertIsNotNone(self.board.guard);self.assertEqual(self.raw()['events'][0]['acks'],['worker:controlled-artifact:fixture-worker'])
  self.settle_fixture_guard();self.command('ack','1:1')
  (self.root/'artifact-1.txt').write_text('tampered')
  with self.assertRaises(SystemExit):self.command('ack','1:1')
  with self.assertRaises(SystemExit):self.command('result','1')
 def test_result_lost_response_reentry_and_no_publication_claim(self):
  self.tick()
  with self.assertRaises(SystemExit):self.command('result','1','--sha','a'*40)
  self.assertNotIn('acceptance_receipts',self.raw())
  update=self.board.update
  def lost(n,**fields):
   update(n,**fields)
   if q.issue_data(self.board.get(n)).get('result'):raise RuntimeError('result response lost after commit')
  with mock.patch.object(self.board,'update',side_effect=lost),self.assertRaises(RuntimeError):self.command('result','1')
  self.assertIsNotNone(self.board.guard);self.settle_fixture_guard()
  before=json.dumps(self.board.issues,sort_keys=True);result=json.loads(self.command('result','1'))
  self.assertTrue(result['task_accepted']);self.assertEqual(before,json.dumps(self.board.issues,sort_keys=True))
 def test_sender_and_wrong_event_ack_refuse_without_effect(self):
  self.tick();before=json.dumps(self.board.issues,sort_keys=True)
  with self.assertRaises(SystemExit):self.command('ack','1:1','--pm',self.actor['session'])
  with self.assertRaises(SystemExit):self.command('ack','1:999')
  self.assertEqual(before,json.dumps(self.board.issues,sort_keys=True))
 def test_stale_lifecycle_preflight_keeps_grants_and_allows_independent_task(self):
  q.CONFIG['capacity']['project_caps']={'slots':2}
  self.project=q.SQLiteCapacity(self.root/'two-slot-project.sqlite','project','fixture/project',{'slots':2})
  self.board.capacity_provider=lambda caps:self.project
  self.command('run','1');self.wait_file();key=self.raw()['lifecycle']['key']
  self.second_task();second=q.issue_data(self.board.get(2));second['execution']['demand']['heavy']=0
  self.board.issues[2]['body']=q.block('second',second)
  raw=self.raw();raw['lifecycle']['artifact']='stale-artifact.txt';self.board.issues[1]['body']=q.block('fixture',raw)
  before=self.board.get(1);host_before=self.host().observe(key);project_before=self.project.observe(key)
  self.tick()
  self.assertEqual(self.board.get(1),before);self.assertEqual(self.host().observe(key),host_before)
  self.assertEqual(self.project.observe(key),project_before)
  self.assertEqual(q.issue_data(self.board.get(2))['lifecycle']['phase'],'parked')
 def test_completed_effect_then_stale_then_eligible_does_not_poison_guard(self):
  import copy
  q.CONFIG['capacity']['project_caps']={'slots':2}
  self.project=q.SQLiteCapacity(self.root/'two-slot-project.sqlite','project','fixture/project',{'slots':2})
  self.board.capacity_provider=lambda caps:self.project
  self.second_task();third=copy.deepcopy(self.board.get(2));raw=q.issue_data(third)
  raw['claim']['session']='third-worker';raw['events'][0]['recipients']=['worker:controlled-artifact:third-worker']
  raw['execution'].update(artifact='third-{event}.txt',demand={'slots':1,'heavy':0})
  third.update(iid=3,body=q.block('third',raw));self.board.issues[3]=third
  first=self.raw();first['execution']['demand']['heavy']=0;self.board.issues[1]['body']=q.block('first',first)
  self.command('run','2');self.wait_file('second-1.txt')
  second=q.issue_data(self.board.get(2));key=second['lifecycle']['key']
  second['lifecycle']['artifact']='stale-artifact.txt';self.board.issues[2]['body']=q.block('second',second)
  before=self.board.get(2);host_before=self.host().observe(key);project_before=self.project.observe(key)
  self.tick()
  self.assertEqual(self.board.get(2),before);self.assertEqual(self.host().observe(key),host_before)
  self.assertEqual(self.project.observe(key),project_before)
  self.assertEqual(self.raw()['lifecycle']['phase'],'parked')
  self.assertEqual(q.issue_data(self.board.get(3))['lifecycle']['phase'],'parked')
  self.assertIsNone(self.board.guard)
 def malformed_ordering(self,change):
  import copy
  self.second_task();third=copy.deepcopy(self.board.get(2));raw=q.issue_data(third)
  raw['claim']['session']='third-worker';raw['events'][0]['recipients']=['worker:controlled-artifact:third-worker']
  raw['execution']['artifact']='third-{event}.txt'
  third.update(iid=3,body=q.block('third',raw));self.board.issues[3]=third
  second=q.issue_data(self.board.get(2));change(second)
  self.board.issues[2]['body']=q.block('second',second);before=self.board.get(2)
  self.tick()
  self.assertEqual(self.board.get(2),before);self.assertEqual(self.raw()['lifecycle']['phase'],'parked')
  self.assertEqual(q.issue_data(self.board.get(3))['lifecycle']['phase'],'parked')
  self.assertIsNone(self.board.guard)
 def test_completed_effect_then_incomplete_input_then_eligible_progresses(self):
  self.malformed_ordering(lambda raw:raw.pop('admission_age'))
 def test_completed_effect_then_escaped_artifact_then_eligible_progresses(self):
  self.malformed_ordering(lambda raw:raw['execution'].update(artifact='../outside'))

 def test_manager_result_native_receipt_uses_existing_supervisor_acceptance(self):
  pm={'runtime':'codex','session':'fixture-manager','name':q.machine()}
  raw=self.raw();raw['pm']=pm;self.board.issues[1]['body']=q.block('fixture',raw)
  self.invoke('admit');self.wait_file();self.invoke('park');self.command('result','1','--text','verified result')
  event=self.raw()['events'][-1]['id'];accepted=json.loads(json.dumps(self.raw()['acceptance_receipts']))
  def apply():return json.loads(self.command('apply-event',f'1:{event}','--role','manager','--artifact','manager-result.txt'))
  with mock.patch.object(q,'session',return_value={k:pm[k] for k in ('runtime','session')}),mock.patch.object(q,'origin',return_value=pm):
   result=apply();before=(self.root/'manager-result.txt').stat().st_mtime_ns
   self.assertEqual(result['phase'],'complete');self.assertFalse(result['task_accepted'])
   self.assertEqual(self.raw()['events'][-1]['acks'],['manager:codex:fixture-manager'])
   self.assertEqual(self.raw()['acceptance_receipts'],accepted)
   saved=next(iter(self.raw()['operations'].values()))['receipt']
   self.assertEqual(saved['receipts'][-1]['stage'],'acceptance')
   self.assertEqual(saved['receipts'][-1]['actor'],'supervisor:codex:fixture-supervisor')
   self.assertEqual(apply(),result);self.assertEqual((self.root/'manager-result.txt').stat().st_mtime_ns,before)
   (self.root/'artifact-1.txt').write_text('changed accepted artifact')
   with self.assertRaises(SystemExit):apply()
  self.assertEqual(self.raw()['acceptance_receipts'],accepted)
 def test_manager_result_without_acceptance_refuses_before_notification_effect(self):
  self.invoke('admit');self.wait_file();self.invoke('park');self.command('result','1')
  raw=self.raw();event=raw['events'][-1]['id'];raw['acceptance_receipts']={}
  self.board.issues[1]['body']=q.block('fixture',raw)
  with mock.patch.object(q,'session',return_value={k:self.actor[k] for k in ('runtime','session')}),self.assertRaises(SystemExit):
   self.command('apply-event',f'1:{event}','--role','manager','--artifact','manager-result.txt')
  self.assertFalse((self.root/'manager-result.txt').exists());self.assertEqual(self.raw()['events'][-1]['acks'],[])


 def test_manager_result_stale_execution_and_fingerprint_refuse_before_effect(self):
  self.invoke('admit');self.wait_file();self.invoke('park');self.command('result','1')
  original=self.raw();event=original['events'][-1]['id']
  for field in ('execution-event','execution-artifact','execution-demand','lifecycle-identity','lifecycle-key','grant-request'):
   with self.subTest(field=field):
    raw=json.loads(json.dumps(original))
    if field=='execution-event':raw['execution']['event']=100
    elif field=='execution-artifact':raw['execution']['artifact']='other-{event}.txt'
    elif field=='execution-demand':raw['execution']['demand']['heavy']=0
    elif field=='lifecycle-identity':raw['lifecycle']['identity']['task']=99
    elif field=='lifecycle-key':raw['lifecycle']['key']='foreign-key'
    else:
     with self.host().transaction() as db:
      value=json.loads(db.execute('SELECT body FROM capacity_lease WHERE id=?',(raw['lifecycle']['key'],)).fetchone()[0])
      value['request']['priority']=0
      db.execute('UPDATE capacity_lease SET body=? WHERE id=?',(json.dumps(value),raw['lifecycle']['key']))
    self.board.issues[1]['body']=q.block('fixture',raw)
    with mock.patch.object(q,'session',return_value={k:self.actor[k] for k in ('runtime','session')}),self.assertRaises(SystemExit):
     self.command('apply-event',f'1:{event}','--role','manager','--artifact','manager-result.txt')
    self.assertFalse((self.root/'manager-result.txt').exists());self.assertEqual(self.raw()['events'][-1]['acks'],[])
 def test_manager_result_acceptance_change_on_final_read_leaves_ack_pending(self):
  self.invoke('admit');self.wait_file();self.invoke('park');self.command('result','1')
  event=self.raw()['events'][-1]['id'];read=q.read_issue;changed=False;after_effect_reads=0
  def changing(n):
   nonlocal changed,after_effect_reads
   if (self.root/'manager-result.txt').exists():after_effect_reads+=1
   if after_effect_reads==5 and not changed:
    changed=True;raw=self.raw();raw['acceptance_criteria']['sha256']='bad'
    self.board.issues[1]['body']=q.block('fixture',raw)
   return read(n)
  with mock.patch.object(q,'session',return_value={k:self.actor[k] for k in ('runtime','session')}),mock.patch.object(q,'read_issue',side_effect=changing),self.assertRaises(SystemExit):
   self.command('apply-event',f'1:{event}','--role','manager','--artifact','manager-result.txt')
  self.assertTrue(changed);self.assertEqual(self.raw()['events'][-1]['acks'],[])
  self.assertNotEqual(next(iter(self.raw()['operations'].values()))['receipt']['phase'],'complete')

 def test_manager_result_consecutive_read_difference_preserves_new_pending_data(self):
  self.invoke('admit');self.wait_file();self.invoke('park');self.command('result','1')
  event=self.raw()['events'][-1]['id'];read=q.read_issue;after_effect_reads=0
  def changing(n):
   nonlocal after_effect_reads
   if (self.root/'manager-result.txt').exists():after_effect_reads+=1
   if after_effect_reads==6:
    raw=self.raw();raw['pending_owner_note']='preserve newest pending data'
    self.board.issues[1]['body']=q.block('fixture',raw)
   return read(n)
  with mock.patch.object(q,'session',return_value={k:self.actor[k] for k in ('runtime','session')}),mock.patch.object(q,'read_issue',side_effect=changing),self.assertRaisesRegex(SystemExit,'changed during verification'):
   self.command('apply-event',f'1:{event}','--role','manager','--artifact','manager-result.txt')
  self.assertEqual(self.raw()['pending_owner_note'],'preserve newest pending data')
  self.assertEqual(self.raw()['events'][-1]['acks'],[])

class Capacity(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=pathlib.Path(self.tmp.name)
  self.provider=q.SQLiteCapacity(self.root/'host.sqlite','host','capacity-test',{'slots':2,'heavy':1})
 def req(self,age,heavy=1):return {'age':age,'priority':9,'demand':{'slots':1,'heavy':heavy},'identity':{'session':str(age)}}
 def test_vector_fairness_light_progress_and_exact_cancel(self):
  self.assertEqual(self.provider.reserve('first',self.req(1))['phase'],'reserved')
  self.assertEqual(self.provider.reserve('old',self.req(2))['phase'],'waiting')
  self.assertEqual(self.provider.reserve('light',self.req(3,0))['phase'],'reserved')
  self.provider.release('first')
  self.assertEqual(self.provider.reserve('young',self.req(4))['phase'],'waiting')
  self.assertEqual(self.provider.reserve('old',self.req(2))['phase'],'reserved')
  self.provider.stop('old');self.provider.release('light')
  self.assertEqual(self.provider.reserve('young',self.req(4))['phase'],'reserved')
 def test_repeated_resume_cannot_jump_an_existing_waiter(self):
  self.provider.reserve('old-task-first-turn',self.req(1))
  self.assertEqual(self.provider.reserve('other-waiter',self.req(2))['phase'],'waiting')
  self.provider.stop('old-task-first-turn')
  self.assertEqual(self.provider.reserve('old-task-resume',self.req(1))['phase'],'waiting')
  self.assertEqual(self.provider.reserve('other-waiter',self.req(2))['phase'],'reserved')
  self.provider.stop('other-waiter')
  self.assertEqual(self.provider.reserve('old-task-resume',self.req(1))['phase'],'reserved')
 def test_identity_caps_and_unknown_birth_refuse(self):
  self.provider.reserve('key',self.req(1));self.provider.launch('key')
  with self.assertRaises(ValueError):self.provider.reserve('key',self.req(2))
  with self.assertRaises(ValueError):self.provider.release('key')
  with self.assertRaises(ValueError):q.SQLiteCapacity(self.root/'host.sqlite','host','capacity-test',{'slots':3,'heavy':1})
  with self.assertRaises(ValueError):self.provider.reserve('impossible',{'age':1,'priority':9,'demand':{'slots':3}})
  self.assertIsNone(self.provider.observe('impossible'))
 def test_competing_process_vector_never_over_admitted(self):
  import subprocess,sys
  code='''import importlib.util,json,sys\ns=importlib.util.spec_from_file_location('q',sys.argv[1]);q=importlib.util.module_from_spec(s);s.loader.exec_module(q)\np=q.SQLiteCapacity(sys.argv[2],'host','capacity-test',{'slots':2,'heavy':1})\nr={'age':int(sys.argv[3]),'priority':9,'demand':{'slots':1,'heavy':1},'identity':{'session':sys.argv[3]}}\nprint(json.dumps(p.reserve(sys.argv[3],r)))'''
  children=[subprocess.Popen([sys.executable,'-c',code,str(pathlib.Path(q.__file__).resolve()),str(self.provider.path),str(i)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True) for i in range(8)]
  results=[]
  for child in children:
   out,err=child.communicate(timeout=10);self.assertEqual(child.returncode,0,err);results.append(json.loads(out))
  self.assertEqual(sum(r['phase']=='reserved' for r in results),1)
  with self.provider.transaction() as db:rows=[json.loads(body) for body, in db.execute('SELECT body FROM capacity_lease')]
  self.assertEqual(sum(v['request']['demand']['heavy'] for v in rows if v['phase']=='reserved'),1)

if __name__=='__main__':unittest.main()
