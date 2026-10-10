"""Identical hermetic board/runtime fixtures; baseline vs candidate actual cmd_status."""
import argparse, contextlib, hashlib, importlib.util, io, json, subprocess, tempfile
from pathlib import Path
from unittest import mock
ROOT=Path(__file__).resolve().parent.parent
BASE='c08678b3e2c0ed80bdcdc008bbcbd41bec6a7c44'
def load(name,path):
 spec=importlib.util.spec_from_file_location(name,path); m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m); return m
candidate=load('candidate',ROOT/'taskq.py')
raw={'pm':{'runtime':'codex','session':'PM','name':'mac'},'claim':{'runtime':'codex','session':'worker','name':'mac'},
 'supervisor':{'runtime':'codex','session':'supervisor','name':'mac'},'deps':[2],
 'event_seq':9,'events':[{'id':9,'recipients':['PM','PM'],'acks':[]}],
 'action_payloads':{'ask':{'id':9,'text':'Current choice'}},'decision':{'summary':'Current choice','options':['retain','qualify']}}
fixture={'iid':1,'title':'Captured obligation','body':candidate.block('goal',raw),'labels':['q-ask'],
 'state':'open','url':'https://fixture/1','updated_at':'2026-10-10','assignees':[]}
gate={'iid':2,'title':'Non-task prerequisite','body':'ordinary issue','labels':[],'state':'open','url':'https://fixture/2','assignees':[]}
class Board:
 def list(self,state): return [fixture,gate]
 def metadata(self,n): return fixture if n==1 else gate
 def get(self,n): return self.metadata(n)
 def observe_guard(self): return {'state':'held','identity':'exact','owner':'mac codex:controller pid=1 uuid'}
 def update(self,*a,**k): raise AssertionError('write')
results={}
with tempfile.TemporaryDirectory() as td:
 path=Path(td)/'baseline.py'; path.write_bytes(subprocess.check_output(['git','show',BASE+':taskq.py'],cwd=ROOT))
 baseline=load('baseline',path)
 for name,module in [('baseline',baseline),('candidate',candidate)]:
  module.CONFIG={'root':Path(td),'repo':'fixture/repo','board':'github'}; module.BOARD=Board()
  kind=mock.Mock(); kind.link.return_value='https://session/worker'
  kind.observe.return_value={'state':'unknown','code':'surface-auth','source':'cli-turn','evidence_id':'fixture-auth'}
  out=io.StringIO()
  with mock.patch.object(module,'runtimes',return_value={'codex':kind}), mock.patch.object(module,'machine',return_value='mac'), contextlib.redirect_stdout(out):
   module.cmd_status(argparse.Namespace(diagnose=True))
  (ROOT/'experiments'/f'{name}-identical-fixture.md').write_text(out.getvalue())
  results[name]={'guard':'Project guard: held' in out.getvalue(),'question':'1.1 retain' in out.getvalue(),
   'pending_event':'pending-event:' in out.getvalue(),'auth':'surface-auth:' in out.getvalue(),
   'ordinary_prerequisite':'dependency:#2' in out.getvalue()}
assert results['candidate']==dict.fromkeys(results['candidate'],True), results
assert results['baseline']['question'] and not results['baseline']['guard'] and not results['baseline']['auth'],results
result={'baseline':BASE,'fixture_sha256':hashlib.sha256(json.dumps([fixture,gate],sort_keys=True).encode()).hexdigest(),'results':results}
(ROOT/'experiments'/'baseline-comparison.json').write_text(json.dumps(result,indent=2)+'\n'); print(json.dumps(result))
