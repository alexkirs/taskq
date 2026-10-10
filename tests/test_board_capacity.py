"""Native issue adapters over fault-injected HTTP fixtures; not live-server qualification."""
import contextlib, importlib.util, json, pathlib
from unittest import mock
spec=importlib.util.spec_from_file_location('board_capacity_fixtures',pathlib.Path(__file__).with_name('test_single.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f);q=f.taskq

class BoardCapacity(f.Base):
 def test_project_metadata_uses_documented_endpoint_without_trailing_slash(self):
  for kind,tool,path in ((q.GitHub,'gh','repos/owner/project'),(q.GitLab,'glab','projects/owner%2Fproject')):
   board=kind('owner/project','board.example')
   with mock.patch.object(q,'run_api',return_value={'id':17}) as api:
    self.assertEqual(board.api('GET',''),{'id':17})
    api.assert_called_once_with(tool,'board.example','GET',path,None)
 def setup_native(self,kind):
  self.kind=kind;self.store=f.FakeBoard()
  self.board=q.BOARD=kind('owner/project','board.example',{'capacity_issue':99,'capacity_project_id':17})
  self.domain={'board':'gitlab' if kind is q.GitLab else 'github','host':'board.example','project_id':17}
  self.value={'schema':1,'domain':self.domain,'caps':{'slots':1},'leases':{}}
  self.body='Keep project notes\n'+self.encode(self.value)+'\nKeep footer'
  self.calls=[];self.lost=False;self.stale=False;self.old=None
  def api(method,path,body=None):
   self.calls.append((method,path,body))
   if method=='GET' and path=='':return {'id':17}
   if method=='GET' and path=='issues/99':
    content=self.old if self.stale and self.old else self.body
    if kind is q.GitLab:return {'iid':99,'title':'Capacity','description':content,'labels':[],
     'state':'opened','updated_at':'now','web_url':'https://board.example/99'}
    return {'number':99,'title':'Capacity','body':content,'labels':[],'state':'open',
     'author_association':'OWNER','updated_at':'now','html_url':'https://board.example/99'}
   if method in ('PATCH','PUT') and path=='issues/99':
    self.old=self.body;self.body=body.get('body',body.get('description'))
    if self.lost:raise RuntimeError('HTTP response lost after committed issue update')
    return {}
   raise AssertionError((method,path))
  self.patches=[mock.patch.object(self.board,'api',side_effect=api),
   mock.patch.object(self.board,'acquire',side_effect=self.store.acquire),
   mock.patch.object(self.board,'release',side_effect=self.store.release)]
  for patch in self.patches:patch.start();self.addCleanup(patch.stop)
  q.CONFIG['repo']='owner/project'
 def encode(self,value):return '<!-- taskq:capacity -->\n```json\n'+json.dumps(value)+'\n```\n<!-- /taskq:capacity -->'
 def request(self,age=1):return {'repo':'owner/project','project_domain':self.domain,'age':age,'priority':2,'demand':{'slots':1}}
 def test_both_native_adapters_reserve_fifo_release_preserve_issue(self):
  for kind in (q.GitHub,q.GitLab):
   with self.subTest(kind=kind):
    self.setup_native(kind)
    with q.coordination():
     p=self.board.capacity_provider({'slots':1})
     self.assertEqual(p.reserve('a',self.request())['phase'],'reserved')
     self.assertEqual(p.reserve('b',self.request(2))['phase'],'waiting')
     p.release('a')
     self.assertEqual(p.reserve('c',self.request(0))['phase'],'waiting')
     self.assertEqual(p.reserve('b',self.request(2))['phase'],'reserved')
     count=len(self.calls);p.reserve('b',self.request(2));self.assertEqual(len(self.calls),count+1)
     p.release('b');p.release('c')
     self.assertIn('Keep project notes',self.body);self.assertTrue(self.body.endswith('Keep footer'))
    self.assertIsNone(self.store.guard)
    for patch in reversed(self.patches):patch.stop()
 def test_no_guard_or_unprovisioned_anchor_refuses_before_write(self):
  self.setup_native(q.GitHub)
  with self.assertRaises(ValueError):self.board.capacity_provider({'slots':1})
  self.assertFalse(self.calls)
  with q.coordination():
   self.board.options['capacity_issue']=None
   with self.assertRaises(ValueError):self.board.capacity_provider({'slots':1})
  self.assertFalse(self.calls)
 def test_wrong_native_project_or_caps_refused(self):
  self.setup_native(q.GitLab)
  with q.coordination():
   self.board.options['capacity_project_id']=18
   with self.assertRaises(ValueError):self.board.capacity_provider({'slots':1})
   self.board.options['capacity_project_id']=17
   p=self.board.capacity_provider({'slots':2})
   with self.assertRaises(ValueError):p.reserve('a',self.request())
  self.assertFalse(any(method in ('PATCH','PUT') for method,_,_ in self.calls))
 def test_missing_host_never_uses_environment_or_cli_default(self):
  for kind in (q.GitHub,q.GitLab):
   self.setup_native(kind);self.board.host=None
   with mock.patch.dict('os.environ',{'GH_HOST':'foreign.example','GITLAB_HOST':'foreign.example'}),q.coordination():
    with self.assertRaisesRegex(ValueError,'explicit server host'):self.board.capacity_provider({'slots':1})
   self.assertFalse(self.calls)
   for patch in reversed(self.patches):patch.stop()
 def test_lost_committed_write_retains_guard_then_exact_readback_reconciles(self):
  self.setup_native(q.GitHub);self.lost=True
  with self.assertRaises(RuntimeError),q.coordination():
   self.board.capacity_provider({'slots':1}).reserve('a',self.request())
  self.assertIsNotNone(self.store.guard)
  writes=sum(method=='PATCH' for method,_,_ in self.calls)
  with self.assertRaises(SystemExit),q.coordination():pass
  self.store.release(self.store.guard) # fixture controller unwound; explicit exact fixture recovery
  self.lost=False
  with q.coordination():
   self.assertEqual(self.board.capacity_provider({'slots':1}).reserve('a',self.request())['phase'],'reserved')
  self.assertEqual(sum(method=='PATCH' for method,_,_ in self.calls),writes)
 def test_stale_readback_poisoned_no_second_effect(self):
  self.setup_native(q.GitLab);self.stale=True
  with self.assertRaises(RuntimeError),q.coordination():
   self.board.capacity_provider({'slots':1}).reserve('a',self.request())
  self.assertIsNotNone(self.store.guard)
 def test_malformed_foreign_or_task_anchor_never_overwritten(self):
  self.setup_native(q.GitHub)
  for value in ({**self.value,'domain':{**self.domain,'host':'foreign'}},
                {**self.value,'leases':{'x':{'phase':'bound'}}}):
   self.body=self.encode(value)
   with q.coordination(),self.assertRaises(ValueError):self.board.capacity_provider({'slots':1}).observe('a')
  self.assertFalse(any(method=='PATCH' for method,_,_ in self.calls))
 def test_overcommitted_duplicate_tickets_or_foreign_requests_refuse(self):
  self.setup_native(q.GitHub)
  first={'request':self.request(),'phase':'reserved','ticket':1}
  for second in ({**first,'ticket':2}, {**first,'phase':'waiting'},
                 {**first,'phase':'waiting','ticket':2,'request':{**self.request(),'project_domain':{'host':'foreign'}}}):
   self.body=self.encode({**self.value,'leases':{'a':first,'b':second}})
   with q.coordination(),self.assertRaises(ValueError):self.board.capacity_provider({'slots':1}).observe('a')
  self.assertFalse(any(method=='PATCH' for method,_,_ in self.calls))
