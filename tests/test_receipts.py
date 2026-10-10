"""Native receipt reducer fault boundaries; no board/network/runtime calls."""
import importlib.util,json,pathlib,unittest
spec=importlib.util.spec_from_file_location('taskq_receipt',pathlib.Path(__file__).parents[1]/'taskq.py')
q=importlib.util.module_from_spec(spec);spec.loader.exec_module(q)
class NativeReceipts(unittest.TestCase):
 def setUp(self):
  q.CONFIG={'root':pathlib.Path('/fixture'),'repo':'fixture/project'}
  self.item={'iid':1,'claim':{'runtime':'fake','session':'worker','name':'host'},'pm':{'runtime':'fake','session':'pm','name':'host'},'supervisor':{'runtime':'fake','session':'supervisor','name':'host'},'raw':{'events':[{'id':1,'action':'answer','text':'owner answer','by':None,'recipients':['worker:fake:worker'],'acks':[]}]}}
 def op(self):return q.ReceiptOperation(self.item,1,'worker:fake:worker')
 def proof(self,op,status='ok'):return {**op.request(),'status':status,'evidence':{'fixture_token':'private test receipt'}}
 def verifier(self,request,receipt):return receipt['evidence']=={'fixture_token':'private test receipt'}
 def test_loss_restart_duplicate_and_completion(self):
  op=self.op()
  for stage in op.stages:
   receipt=self.proof(op);self.assertTrue(op.receipt(receipt,self.verifier))
   op=q.ReceiptOperation.restore(op.snapshot(),self.item,self.verifier)
   before=op.snapshot();self.assertFalse(op.receipt(receipt,self.verifier));self.assertEqual(before,op.snapshot())
  self.assertEqual(op.phase,'complete')
 def test_order_wrong_actor_and_conflicting_duplicate(self):
  for key,value in [('stage','application'),('actor','sender'),('target','other'),('operation','other')]:
   op=self.op();before=op.snapshot();receipt=self.proof(op);receipt[key]=value
   with self.assertRaises(ValueError):op.receipt(receipt,self.verifier)
   self.assertEqual(before,op.snapshot())
  op=self.op();receipt=self.proof(op);op.receipt(receipt,self.verifier);receipt['status']='unknown'
  with self.assertRaises(ValueError):op.receipt(receipt,self.verifier)
 def test_self_asserted_verified_is_not_authentication(self):
  op=self.op();receipt=self.proof(op);receipt['evidence']={'source':'qualified-runtime','verified':True}
  with self.assertRaises(ValueError):op.receipt(receipt,self.verifier)
  self.assertEqual(op.phase,'pending')
  with self.assertRaises(ValueError):op.receipt(self.proof(op),None)
 def test_stale_answer_identity_and_repo_refused_on_restart(self):
  for change in ('answer','identity','repo'):
   op=self.op();saved=op.snapshot();item=json.loads(json.dumps(self.item))
   if change=='answer':item['raw']['events'][0]['text']='changed answer'
   elif change=='identity':item['claim']['session']='replacement'
   else:q.CONFIG['repo']='another/project'
   with self.assertRaises(ValueError):q.ReceiptOperation.restore(saved,item,self.verifier)
   q.CONFIG['repo']='fixture/project'
 def test_result_requires_distinct_acceptance_and_artifact_verifier(self):
  self.item['raw']['events'][0]['action']='result';op=self.op()
  for stage in op.stages[:-1]:op.receipt(self.proof(op),self.verifier)
  self.assertEqual(op.phase,'pending');self.assertEqual(op.request()['actor'],'supervisor:fake:supervisor')
  receipt=self.proof(op)
  with self.assertRaises(ValueError):op.receipt(receipt,lambda request,value:False)
  op.receipt(receipt,self.verifier);self.assertEqual(op.phase,'complete')
 def test_unknown_decline_never_advance(self):
  for status in ('unknown','declined'):
   op=self.op();op.receipt(self.proof(op,status),self.verifier);saved=op.snapshot()
   restored=q.ReceiptOperation.restore(saved,self.item,self.verifier);self.assertEqual(restored.phase,'blocked')
   with self.assertRaises(ValueError):restored.request()
 def test_ack_changes_do_not_invent_new_operation(self):
  op=self.op();self.item['raw']['events'][0]['acks']=['worker:fake:worker'];self.assertEqual(op.id,self.op().id)
 def test_obsolete_recipient_does_not_authorize_new_worker(self):
  self.item['claim']['session']='replacement'
  with self.assertRaises(ValueError):self.op()
 def test_malformed_event_recipient_and_boolean_id_rejected(self):
  event=self.item['raw']['events'][0]
  event['recipients']='worker:fake:worker'
  with self.assertRaises(ValueError):self.op()
  event['recipients']=['worker:fake:worker'];event['id']=True
  with self.assertRaises(ValueError):self.op()
 def test_tampered_checkpoint_and_duplicate_event_refused(self):
  op=self.op();saved=op.snapshot();saved['phase']='complete'
  with self.assertRaises(ValueError):q.ReceiptOperation.restore(saved,self.item,self.verifier)
  self.item['raw']['events'].append(dict(self.item['raw']['events'][0]))
  with self.assertRaises(ValueError):self.op()
if __name__=='__main__':unittest.main()
