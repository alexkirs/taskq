"""Normal CLI + real detached dispatch on one disposable board; no external API/model turns."""
import hashlib,importlib.util,json,os,pathlib,subprocess,sys,time,uuid
from lifecycle_native_probe import BOARD_SOURCE,TASKQ,q

AUDIT='''
native_acquire=acquire
native_release=release
def acquire(owner):
 token=native_acquire(owner)
 if token:
  with connect() as db:
   db.execute('CREATE TABLE IF NOT EXISTS audit (token TEXT PRIMARY KEY,pid INTEGER,birth TEXT,released INTEGER)')
   db.execute('INSERT INTO audit VALUES (?,?,?,0)',(token,os.getpid(),q.process_identity(os.getpid())[1]))
 return token
def release(token):
 native_release(token)
 with connect() as db:db.execute('UPDATE audit SET released=1 WHERE token=?',(token,))
'''

def main():
 root=pathlib.Path(sys.argv[1]).resolve();root.mkdir(parents=True,exist_ok=False);os.chmod(root,0o700)
 actor=q.origin()
 if not actor or not actor.get('session'):raise SystemExit('genuine caller identity required')
 (root/'board.py').write_text('TASKQ='+repr(str(TASKQ))+'\n'+BOARD_SOURCE+AUDIT)
 (root/'taskq.json').write_text(json.dumps({'board':'board.py','repo':'isolated/queue-integration','update':False,
  'limits':{'claude':0,'codex':6},'capacity':{'project_caps':{'slots':1},'host_caps':{'slots':2,'heavy':1},
  'host_path':str(root/'host.sqlite')}}))
 board=q.load_file('board.py',root);commands=[];observations=[]
 def read(n):return q.issue_data(board.get(n))
 def add(n):
  sid='controlled-'+str(uuid.uuid4());text=f'finite task {n}\n'
  raw={'event_schema':2,'event_seq':1,'events':[{'id':1,'action':'answer','text':text,'by':actor,
    'recipients':['worker:controlled-artifact:'+sid],'acks':[]}],
   'claim':{'runtime':'controlled-artifact','session':sid,'name':q.machine()},'supervisor':actor,'pm':actor,
   'order':{'text':'finite task only'},'history':[{'kind':'original'}],'admission_age':n,'deps':[],
   'execution':{'kind':'controlled-artifact','event':1,'artifact':f'task{n}-{{event}}.txt','demand':{'slots':1,'heavy':1}},
   'acceptance_criteria':{'kind':'answer-artifact','event':1,'artifact':f'task{n}-1.txt','sha256':hashlib.sha256(text.encode()).hexdigest()}}
  issue={'iid':n,'title':'Finite isolated queue task','labels':['q-ready'],'state':'open','body':q.block('synthetic',raw),
         'updated_at':'2026-10-10T00:00:00Z','comments':[]}
  with board.connect() as db:db.execute('INSERT INTO issues VALUES (?,?)',(n,json.dumps(issue)))
 def invoke(*args,expected=0):
  command=[sys.executable,'-B',str(TASKQ),*args]
  child=subprocess.Popen(command,cwd=root,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
  birth=q.process_identity(child.pid)[1];out,err=child.communicate(timeout=40)
  commands.append({'argv':command,'pid':child.pid,'birth':birth,'exit':child.returncode,'stdout':out,'stderr':err})
  assert (child.returncode==0) if expected==0 else (child.returncode!=0),(command,out,err)
 def settle(n,phase='parked'):
  deadline=time.monotonic()+20
  while time.monotonic()<deadline:
   with board.connect() as db:
    guard=db.execute('SELECT token FROM guard').fetchone()
    audit=db.execute('SELECT pid,birth,released FROM audit').fetchall()
   raw=read(n)
   if not guard and q.parse(board.get(n))['state']!='ready' and (raw.get('lifecycle') or {}).get('phase')==phase and all(r[2] and (r[0]==os.getpid() or q.process_state(r[0],r[1])=='dead') for r in audit):
    observations.append({'task':n,'fresh_board':board.get(n),'controllers':audit});return raw
   time.sleep(.05)
  raise AssertionError('own fixture dispatch/drain not settled')
 try:
  add(1);original={k:read(1)[k] for k in ('claim','supervisor','pm','order','history')}
  invoke('run','1');first=settle(1)
  assert first['events'][0]['acks']==first['events'][0]['recipients']
  invoke('ack','1:1');before=board.get(1);invoke('ack','1:1');assert board.get(1)==before
  invoke('answer','1','--text','second exact answer\n');second=settle(1)
  assert second['lifecycle']['generation']==2 and second['claim']==original['claim']
  assert (root/'task1-2.txt').read_bytes()==b'second exact answer\n'
  assert second['events'][1]['acks']==second['events'][1]['recipients']
  invoke('result','1',expected=1);assert read(1).get('result') is None
  token=board.acquire('own disposable acceptance criteria update');assert token
  raw=read(1);raw['acceptance_criteria']={'kind':'answer-artifact','event':2,'artifact':'task1-2.txt',
   'sha256':hashlib.sha256(b'second exact answer\n').hexdigest()}
  board.update(1,body=q.block('synthetic',raw));board.release(token)
  invoke('result','1','--text','bounded artifact independently accepted');accepted=settle(1)
  assert accepted['result']['kind']=='answer-artifact' and q.parse(board.get(1))['state']=='review'
  assert accepted['lifecycle']['key'] in accepted['acceptance_receipts']
  result_event=accepted['events'][-1]['id']
  invoke('apply-event',f'1:{result_event}','--role','manager','--artifact','manager-result.txt')
  assert read(1)['events'][-1]['acks']==[q.recipient('manager',actor)]
  before=board.get(1);invoke('apply-event',f'1:{result_event}','--role','manager','--artifact','manager-result.txt');assert board.get(1)==before
  invoke('ack',f'1:{result_event}','--pm',actor['session'],expected=1);assert board.get(1)==before
  before=board.get(1);invoke('result','1');settle(1);assert board.get(1)==before
  invoke('run','1',expected=1);invoke('answer','1','--text','no implicit rework',expected=1)
  add(2);invoke('tick','--quiet');settle(2)
  invoke('ask','2','--text','select continuation','--option','third exact answer','--recommend','1')
  invoke('tick','--quiet');assert q.parse(board.get(2))['state']=='ask'
  ask_event=read(2)['events'][-1]['id']
  invoke('apply-event',f'2:{ask_event}','--role','manager','--artifact','manager-question.txt')
  assert read(2)['events'][-1]['acks']==[q.recipient('manager',actor)]
  invoke('answer','2.1');third=settle(2)
  assert third['lifecycle']['generation']==2 and third['events'][-1]['acks']==third['events'][-1]['recipients']
  assert (root/'task2-3.txt').read_text()=='2.1: third exact answer'
  with board.connect() as db:
   assert db.execute('SELECT count(*) FROM guard').fetchone()[0]==0
  host=q.SQLiteCapacity(root/'host.sqlite','host',q.machine(),{'slots':2,'heavy':1})
  with host.transaction() as db:leases=[json.loads(body) for body, in db.execute('SELECT body FROM capacity_lease')]
  assert len(leases)==4 and all(v['phase']=='released' and host.drain_verified(v) for v in leases)
  final=[board.get(n) for n in (1,2)]
  for k,v in original.items():assert read(1)[k]==v
  result={'passed':True,'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=TASKQ.parent,text=True).strip(),
   'source_sha256':hashlib.sha256(TASKQ.read_bytes()).hexdigest(),'actor':actor,'commands':commands,
   'observations':observations,'final_board_readback':final,'host_leases':leases,'all_children_dead':True,
   'scope':'normal TaskQ CLI and detached dispatch, disposable SQLite board, controlled-artifact only; no native Codex drain or external provider rerun'}
  (root/'results.json').write_text(json.dumps(result,indent=2));print(json.dumps({'passed':True,'commands':len(commands),'results':str(root/'results.json')}))
 except BaseException as error:
  (root/'failure.json').write_text(json.dumps({'passed':False,'error':str(error),'commands':commands,'observations':observations},indent=2))
  raise

if __name__=='__main__':main()
