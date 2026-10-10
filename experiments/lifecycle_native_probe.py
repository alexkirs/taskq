"""Bounded native lifecycle probe. Requires the caller's genuine TaskQ session identity.
Only fresh disposable SQLite boards/finite controlled children; no production configuration.
"""
import hashlib,importlib.util,json,os,pathlib,re,sqlite3,subprocess,sys,time,uuid
TASKQ=pathlib.Path(__file__).resolve().parents[1]/'taskq.py'
spec=importlib.util.spec_from_file_location('probe_taskq',TASKQ);q=importlib.util.module_from_spec(spec);spec.loader.exec_module(q)
BOARD_SOURCE='''import contextlib,importlib.util,json,os,pathlib,sqlite3,time,uuid
ROOT=pathlib.Path(__file__).parent
spec=importlib.util.spec_from_file_location('capacity_core',TASKQ)
q=importlib.util.module_from_spec(spec);spec.loader.exec_module(q)
DB=ROOT/'board.sqlite';updates=0
@contextlib.contextmanager
def connect():
 with contextlib.closing(sqlite3.connect(DB,timeout=5)) as db,db:
  db.execute('PRAGMA synchronous=FULL')
  db.execute('CREATE TABLE IF NOT EXISTS issues (id INTEGER PRIMARY KEY,body TEXT)')
  db.execute('CREATE TABLE IF NOT EXISTS guard (slot INTEGER PRIMARY KEY CHECK(slot=1),token TEXT,owner TEXT)')
  yield db
def fault(kind,when):
 p=ROOT/'fault.json'
 if p.exists():
  f=json.loads(p.read_text())
  if f['kind']==kind and f['when']==when and (kind!='update' or f['count']==updates):
   time.sleep(.05);os._exit(99)
def get(n):
 with connect() as db:return json.loads(db.execute('SELECT body FROM issues WHERE id=?',(n,)).fetchone()[0])
def list(state):
 with connect() as db:values=[json.loads(r[0]) for r in db.execute('SELECT body FROM issues')]
 return [v for v in values if v['state']=='open' and (state is None or 'q-'+state in v['labels'])]
def closed():
 with connect() as db:return [json.loads(r[0]) for r in db.execute('SELECT body FROM issues') if json.loads(r[0])['state']=='closed']
def update(n,labels=None,body=None):
 global updates
 updates+=1;fault('update','before');v=get(n)
 if labels is not None:v['labels']=labels
 if body is not None:v['body']=body
 with connect() as db:db.execute('UPDATE issues SET body=? WHERE id=?',(json.dumps(v),n))
 fault('update','after')
def acquire(owner):
 with connect() as db:
  token=str(uuid.uuid4())
  try:db.execute('INSERT INTO guard VALUES (1,?,?)',(token,owner))
  except sqlite3.IntegrityError:return None
  return token
def release(token):
 with connect() as db:
  if db.execute('DELETE FROM guard WHERE slot=1 AND token=?',(token,)).rowcount!=1:raise RuntimeError('exact fixture token required')
class ProjectCapacity(q.SQLiteCapacity):
 def release(self,key):
  fault('project-release','before');value=super().release(key);fault('project-release','after');return value
def capacity_provider(caps):return ProjectCapacity(DB,'project',json.loads((ROOT/'taskq.json').read_text())['repo'],caps)
'''

def main():
 if len(sys.argv)!=2:raise SystemExit('pass one fresh output directory')
 root=pathlib.Path(sys.argv[1]).resolve();root.mkdir(parents=True,exist_ok=False);os.chmod(root,0o700)
 actor=q.origin()
 if not actor or not actor.get('session'):raise SystemExit('genuine current runtime session required; never spoof identity')
 caps={'slots':3,'heavy':1};host=q.SQLiteCapacity(root/'host.sqlite','host',q.machine(),caps)
 projects={};commands=[];snapshots=[];fault_receipts=[]
 def make_project(name):
  path=root/name;path.mkdir();repo='isolated/'+name
  (path/'board.py').write_text('TASKQ='+repr(str(TASKQ))+'\n'+BOARD_SOURCE)
  (path/'taskq.json').write_text(json.dumps({'board':'board.py','repo':repo,'update':False,'limits':{'claude':0,'codex':6},
   'capacity':{'project_caps':{'slots':2},'host_caps':caps,'host_path':str(host.path)}}))
  board=q.load_file('board.py',path);projects[name]=(path,board);return board
 def task(project,n,age,heavy=1):
  board=projects[project][1];sid='controlled-'+str(uuid.uuid4());text='qualification '+project+' '+str(n)+'\n'
  raw={'event_schema':2,'event_seq':1,'events':[{'id':1,'action':'answer','text':text,'by':actor,'recipients':['worker:controlled-artifact:'+sid],'acks':[]}],
   'claim':{'runtime':'controlled-artifact','session':sid,'name':q.machine()},'supervisor':actor,'pm':actor,'order':{'text':'finite qualification only'},
   'history':[{'kind':'original'}],'admission_age':age,'execution':{'kind':'controlled-artifact','event':1,'artifact':'task'+str(n)+'-event{event}.txt','demand':{'slots':1,'heavy':heavy}},
   'acceptance_criteria':{'kind':'answer-artifact','event':1,'artifact':'task'+str(n)+'-event1.txt','sha256':hashlib.sha256(text.encode()).hexdigest()}}
  issue={'iid':n,'title':'Disposable '+project+' '+str(n),'labels':['q-ready'],'state':'open','body':q.block('fixture',raw),'updated_at':'2026-10-10T00:00:00Z','comments':[]}
  with board.connect() as db:db.execute('INSERT INTO issues VALUES (?,?)',(n,json.dumps(issue)))
 def invariants(label):
  with host.transaction() as db:rows=[(k,json.loads(body)) for k,body in db.execute('SELECT id,body FROM capacity_lease')]
  used={r:sum(v['request']['demand'].get(r,0) for k,v in rows if v['phase'] not in ('waiting','released')) for r in caps}
  assert all(used[r]<=caps[r] for r in caps),used
  project_used={}
  for name,(path,board) in projects.items():
   provider=board.capacity_provider({'slots':2})
   with provider.transaction() as db:pairs={k:json.loads(body) for k,body in db.execute('SELECT id,body FROM capacity_lease')}
   project_used[name]=sum(v['request']['demand']['slots'] for v in pairs.values() if v['phase'] not in ('waiting','released'))
   assert project_used[name]<=2
   for key,value in rows:
    if value['request']['repo']=='isolated/'+name and value['phase']=='bound':assert pairs[key]['phase']=='reserved'
  snapshots.append({'label':label,'host_used':used,'project_used':project_used})
 def invoke(project,n,action,text=None,expected=0,fault=None):
  path,board=projects[project];fp=path/'fault.json'
  if fault:fp.write_text(json.dumps(fault))
  command=[sys.executable,str(TASKQ),'lifecycle',str(n),action]+(['--text',text] if text is not None else [])
  start=time.time();process=subprocess.Popen(command,cwd=path,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
  state,birth=q.process_identity(process.pid);out,err=process.communicate(timeout=15)
  commands.append({'project':project,'task':n,'action':action,'command':command,'exit':process.returncode,'stdout':out,'stderr':err,
                   'pid':process.pid,'birth':birth,'elapsed':time.time()-start})
  assert process.returncode==expected,(action,process.returncode,out,err)
  if fault:
   fp.unlink();assert q.process_state(process.pid,birth)=='dead'
   with board.connect() as db:row=db.execute('SELECT token,owner FROM guard WHERE slot=1').fetchone()
   assert row and 'pid='+str(process.pid)+' ' in row[1]
   # Exact own fixture controller is dead; no other board writers were created by this driver.
   # Controlled children access host grants/artifacts only, never this board.
   board.release(row[0]);fault_receipts.append({'fault':fault,'pid':process.pid,'birth':birth,'controller_dead':True,'exact_fixture_guard':row[0]})
  invariants(project+':'+str(n)+':'+action+(' crash' if fault else ''))
  return json.loads(out) if out.strip() else None
 def raw(project,n):return q.issue_data(projects[project][1].get(n))
 def wait_artifact(project,n,event=1):
  path=projects[project][0]/('task'+str(n)+'-event'+str(event)+'.txt');end=time.monotonic()+3
  while not path.exists() and time.monotonic()<end:time.sleep(.01)
  assert path.exists(),str(path)
 try:
  for name in ('A','B','C'):make_project(name)
  task('A',1,1);task('A',2,2,0);task('A',3,3,0);task('B',1,4);task('C',1,5)
  a=invoke('A',1,'admit');assert a['phase']=='active';wait_artifact('A',1)
  assert invoke('A',2,'admit')['phase']=='active';wait_artifact('A',2)
  assert invoke('A',3,'admit')['resource']=='project' # host still has a free slot
  invoke('A',3,'park') # explicit cancellation of the earlier project waiter
  assert invoke('B',1,'admit')['resource']=='host'
  assert invoke('C',1,'admit')['resource']=='host'
  assert invoke('A',1,'resume')['child']==a['child']
  invoke('A',1,'park');assert invoke('A',1,'accept')['task_accepted']
  assert invoke('A',1,'resume')['resource']=='host' # original age cannot jump existing waiters
  invoke('A',1,'park') # cancel its pending grants without a child launch
  assert invoke('C',1,'reconcile')['phase']=='waiting' # oldest eligible B wins
  assert invoke('B',1,'reconcile')['phase']=='active';wait_artifact('B',1)
  assert invoke('A',3,'resume')['phase']=='active';wait_artifact('A',3)
  assert snapshots[-1]['host_used']=={'slots':3,'heavy':1}
  invoke('B',1,'park');assert invoke('C',1,'reconcile')['phase']=='active';wait_artifact('C',1)
  invoke('C',1,'park');invoke('A',2,'park');invoke('A',3,'park')
  original=raw('A',1);invoke('A',1,'answer','answer received while parked\n')
  assert raw('A',1)['events'][-1]['acks']==[]
  resumed=invoke('A',1,'resume');wait_artifact('A',1,2);assert invoke('A',1,'resume')['child']==resumed['child']
  for field in ('claim','supervisor','pm','history','order'):assert raw('A',1)[field]==original[field]
  assert raw('A',1)['lifecycle']['age']==original['lifecycle']['age']
  faults=[{'kind':'update','count':2,'when':'before'},{'kind':'update','count':2,'when':'after'},
          {'kind':'project-release','when':'before'},{'kind':'project-release','when':'after'},
          {'kind':'update','count':3,'when':'before'},{'kind':'update','count':3,'when':'after'}]
  for fault in faults:
   invoke('A',1,'park',expected=99,fault=fault)
   assert invoke('A',1,'park')['phase']=='parked'
   assert raw('A',1)['events'][-1]['acks']==['worker:controlled-artifact:'+original['claim']['session']]
   if fault!=faults[-1]:invoke('A',1,'resume');wait_artifact('A',1,2)
  # Actual controller dies after native child start but before its board response is recorded.
  invoke('A',1,'resume',expected=99,fault={'kind':'update','count':3,'when':'before'})
  key=raw('A',1)['lifecycle']['key'];child=host.observe(key)['child']
  assert invoke('A',1,'resume')['child']==child;invoke('A',1,'park')
  # Explicit new answer-specific criteria, not automatic acceptance of a generic product task.
  board=projects['A'][1];issue=board.get(1);data=q.issue_data(issue)
  data['acceptance_criteria']={'kind':'answer-artifact','event':2,'artifact':'task1-event2.txt','sha256':hashlib.sha256(b'answer received while parked\n').hexdigest()}
  token=board.acquire('owned fixture acceptance criteria update');assert token
  board.update(1,body=q.block('fixture',data));board.release(token)
  assert invoke('A',1,'accept')['task_accepted'];assert board.get(1)['state']=='open';assert raw('A',1).get('result') is None
  invariants('final');assert snapshots[-1]['host_used']=={'slots':0,'heavy':0};assert not any(snapshots[-1]['project_used'].values())
  with host.transaction() as db:leases=[json.loads(body) for body, in db.execute('SELECT body FROM capacity_lease')]
  assert all(v['phase']=='released' for v in leases)
  assert all(q.process_state(v['child']['pid'],v['child']['birth'])=='dead' for v in leases if v.get('child'))
  result={'passed':True,'base_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=TASKQ.parent,text=True).strip(),
          'source_sha256':hashlib.sha256(TASKQ.read_bytes()).hexdigest(),'actor':actor,'commands':commands,'snapshots':snapshots,
          'fault_receipts':fault_receipts,'all_grants_released':True,'all_controlled_children_dead':True,
          'scope':'SQLite qualification boards and controlled-artifact runtime; not native Codex/app drain or GitHub/GitLab activation'}
  (root/'results.json').write_text(json.dumps(result,indent=2));print(json.dumps({'passed':True,'commands':len(commands),'faults':len(fault_receipts),'results':str(root/'results.json')}))
 except BaseException as error:
  (root/'failure-results.json').write_text(json.dumps({'passed':False,'failure':type(error).__name__,
    'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=TASKQ.parent,text=True).strip(),
    'source_sha256':hashlib.sha256(TASKQ.read_bytes()).hexdigest(),'commands':commands,'snapshots':snapshots,'fault_receipts':fault_receipts},indent=2))
  raise
 finally:
  for name,(path,board) in projects.items():
   for issue in board.list(None):
    data=q.issue_data(issue)
    if data.get('lifecycle') and data['lifecycle']['phase']!='parked':
     token=None
     try:
      with board.connect() as db:row=db.execute('SELECT token,owner FROM guard WHERE slot=1').fetchone()
      if row:continue # never clean up a holder not proven to be this completed controller
      invoke(name,issue['iid'],'park')
     except BaseException as e:print('fixture cleanup unresolved:',name,issue['iid'],type(e).__name__,file=sys.stderr)

if __name__=='__main__':main()
