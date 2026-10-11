"""Isolated service contract tests: real SQLite/IPC/child lifecycle, no provider or model."""
import contextlib
import sqlite3
import importlib.util
import json
import multiprocessing.connection as ipc
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock
import host_service as h

class ServiceTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.home=Path(self.temp.name).resolve();self.db=h.connect_db(self.home/'registry.sqlite');self.addCleanup(self.db.close)
        self.state={'version':1,'project':str(self.home),'epoch':'epoch1','phase':'paused','stop':False,'operation':None}
        h.save_state(self.db,self.state);self.db.commit()
    def value(self,operation='resume',id='one'):
        return {'version':1,'id':id,'epoch':'epoch1','operation':operation}
    def test_dispatch_model_paused_wake_and_fallback(self):
        for phase in ('paused','staged','legacy'):
            self.assertFalse(h.dispatch_ready(phase,10,0,True))
        self.assertTrue(h.dispatch_ready('active',10,10,False))
        self.assertTrue(h.dispatch_ready('active',10,180,True))
        self.assertFalse(h.dispatch_ready('active',10,180,False))
    def test_wakes_defer_unknown_birth_coalesce_and_preserve_new_events(self):
        h.private_home(self.home,created=True)
        folder=self.home/'.taskq';folder.mkdir()
        with contextlib.closing(h.connect_db(folder/'dispatch-owner.sqlite')) as owner:
            h.save_state(owner,{'version':1,'phase':'owned','epoch':'epoch1','registry':str(self.home/'registry.sqlite')});owner.commit()
        (self.home/'taskq.json').write_bytes(b'{}')
        state=h.read_state(self.db);state.update(project=str(self.home),config_sha256=h.hashlib.sha256(b'{}').hexdigest())
        h.save_state(self.db,state);self.db.commit()
        fake=mock.Mock();fake.process_state.return_value='unknown'
        h.notify(self.home,[1]);h.notify(self.home,[2]);h.notify(self.home,[],123,'birth1')
        self.assertEqual(2,self.db.execute('SELECT count(*) FROM wakes').fetchone()[0])
        self.assertEqual((False,[]),h.take_wakes(fake,self.db,self.home))
        state['phase']='active';h.save_state(self.db,state);self.db.commit()
        self.assertEqual((True,[1,2]),h.take_wakes(fake,self.db,self.home))
        self.assertEqual((False,[]),h.take_wakes(fake,self.db,self.home))
        fake.process_state.return_value='running'
        self.assertEqual((False,[]),h.take_wakes(fake,self.db,self.home))
        fake.process_state.return_value='dead'
        self.assertEqual((True,[]),h.take_wakes(fake,self.db,self.home))
        h.notify(self.home,[3])  # a mutation while a pass runs survives for the subsequent pass
        self.assertEqual((True,[3]),h.take_wakes(fake,self.db,self.home))
        self.assertEqual((False,[]),h.take_wakes(fake,self.db,self.home))
        with self.assertRaisesRegex(ValueError,'PID/birth'):h.notify(self.home,[],123,None)
        (self.home/'taskq.json').write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError,'config changed'):h.notify(self.home,[4])
    def test_wake_stale_epoch_malformed_and_bound_fail_closed(self):
        state=h.read_state(self.db);state['phase']='active';h.save_state(self.db,state);self.db.commit()
        fake=mock.Mock();fake.process_state.return_value='dead'
        value={'epoch':'old','tasks':[1],'after':None,'birth':None}
        key=h.digest({'epoch':'old','pid':None,'birth':None})
        self.db.execute('INSERT INTO wakes VALUES (?,?)',(key,json.dumps(value)));self.db.commit()
        with mock.patch.object(h,'validate_owner'):
            self.assertEqual((False,[]),h.take_wakes(fake,self.db,self.home))
            self.db.execute('INSERT INTO wakes VALUES (?,?)',('bad','{}'));self.db.commit()
            with self.assertRaisesRegex(ValueError,'wake'):h.take_wakes(fake,self.db,self.home)
            self.assertEqual(2,self.db.execute('SELECT count(*) FROM wakes').fetchone()[0])
    def test_mutation_replay_and_conflict_persisted(self):
        first=h.request(None,self.db,self.value());self.assertEqual(first,h.request(None,self.db,self.value()))
        self.db.close();self.db=h.connect_db(self.home/'registry.sqlite');self.addCleanup(self.db.close)
        self.assertEqual(first,h.request(None,self.db,self.value()))
        with self.assertRaisesRegex(ValueError,'conflicting'): h.request(None,self.db,self.value('pause'))
        self.assertEqual('active',h.read_state(self.db)['phase'])
    def test_registry_initialization_failure_closes_connection(self):
        connection=mock.Mock()
        connection.execute.side_effect=sqlite3.OperationalError('database is locked')
        with mock.patch.object(h.sqlite3,'connect',return_value=connection):
            with self.assertRaisesRegex(sqlite3.OperationalError,'locked'):
                h.connect_db(self.home/'failed.sqlite')
        connection.close.assert_called_once_with()

    def test_busy_wake_registry_diagnostic_cannot_kill_source(self):
        with contextlib.closing(sqlite3.connect(self.home/'registry.sqlite',timeout=0)) as writer:
            writer.execute('BEGIN IMMEDIATE')
            with mock.patch('sys.stderr'):
                h.record_error(self.db,sqlite3.OperationalError('busy wake writer'))
            self.assertFalse(self.db.in_transaction)
            writer.rollback()
        self.assertEqual('paused',h.read_state(self.db)['phase'])

    def test_mailbox_failure_and_read_requests_do_not_starve_due_pass(self):
        fake=mock.Mock();fake.CLONE=self.home;fake.SESSIONS={}
        (self.home/'current.json').write_text(json.dumps({'path':str(self.home),'commit':self.home.name}))
        for queued in (False,True):
            with self.subTest(queued=queued):
                state=h.read_state(self.db);state.update(phase='active',stop=False,install_dir=str(self.home))
                h.save_state(self.db,state);self.db.commit()
                pending=mock.Mock()
                pending.get.side_effect=None if queued else h.queue.Empty
                pending.get.return_value=(mock.Mock(),self.value('status'))
                def finished(*args):
                    state=h.read_state(self.db);state['stop']=True;h.save_state(self.db,state);self.db.commit()
                with mock.patch.object(h,'owner_lock',return_value=contextlib.nullcontext()), \
                        mock.patch.object(h,'validate_owner'),mock.patch.object(h.ipc,'Listener'), \
                        mock.patch.object(h.os,'chmod'), \
                        mock.patch.object(h.threading,'Thread'),mock.patch.object(h.queue,'Queue',return_value=pending), \
                        mock.patch.object(h,'request',return_value={'outcome':'observed'}), \
                        mock.patch.object(h,'take_wakes',side_effect=ValueError('malformed wake')), \
                        mock.patch.object(h,'run_pass',side_effect=finished) as run, \
                        mock.patch.dict(os.environ,{},clear=True),mock.patch('sys.stderr'):
                    h.serve(fake,self.db,self.home,180)
                run.assert_called_once_with(fake,self.db,self.home,[])

    def test_cache_write_failure_rolls_back_control_state(self):
        self.db.execute("CREATE TRIGGER reject_receipt BEFORE INSERT ON requests BEGIN SELECT RAISE(ABORT,'receipt unavailable'); END")
        self.db.commit()
        with self.assertRaisesRegex(h.sqlite3.IntegrityError,'receipt unavailable'):h.request(None,self.db,self.value())
        self.assertEqual('paused',h.read_state(self.db)['phase'])
        self.assertEqual(0,self.db.execute('SELECT count(*) FROM requests').fetchone()[0])
    def test_busy_unknown_and_stale_refuse_without_changes(self):
        state=h.read_state(self.db)
        for phase in ('intent','running'):
            state['operation']={'phase':phase};h.save_state(self.db,state);self.db.commit()
            with self.assertRaisesRegex(ValueError,'busy'): h.request(None,self.db,self.value())
        with self.assertRaisesRegex(ValueError,'stale'): h.request(None,self.db,{**self.value(),'epoch':'old'})
        self.assertEqual(state,h.read_state(self.db))
        with self.assertRaisesRegex(ValueError,'schema'):h.request(None,self.db,{**self.value(),'session':'spoof'})
    def test_native_admission_exact_epoch_pid_birth_and_completion_boundary(self):
        project=self.home/'.taskq';project.mkdir();owner=h.connect_db(project/'dispatch-owner.sqlite')
        state=h.read_state(self.db);state.update(phase='active',operation={'phase':'running','child':{'pid':123,'birth':'native:1'}});h.save_state(self.db,state);self.db.commit()
        h.save_state(owner,{'version':1,'phase':'owned','epoch':'epoch1','registry':str(self.home/'registry.sqlite')});owner.commit();owner.close()
        with mock.patch.dict(os.environ,{'TASKQ_SERVICE_EPOCH':'epoch1'}):
            h.gate(self.home,{'pid':123,'birth':'native:1'})
            for identity in (None,{'pid':123,'birth':'native:2'},{'pid':124,'birth':'native:1'}):
                with self.assertRaises(ValueError):h.gate(self.home,identity)
        with mock.patch.dict(os.environ,{'TASKQ_SERVICE_EPOCH':'old'}):
            with self.assertRaises(ValueError): h.gate(self.home,{'pid':123,'birth':'native:1'})
        state['operation']['phase']='complete';h.save_state(self.db,state);self.db.commit()
        with self.assertRaises(ValueError):h.gate(self.home,{'pid':123,'birth':'native:1'})
    def test_same_os_user_json_ipc_and_exclusive_source(self):
        address,family=h.endpoint(self.home);listener=ipc.Listener(address,family=family)
        self.addCleanup(listener.close);seen=[]
        def client():
            conn=ipc.Client(address,family=family);h.peer_user(conn,server=True);conn.send_bytes(json.dumps(self.value()).encode());seen.append(json.loads(conn.recv_bytes()));conn.close()
        thread=threading.Thread(target=client);thread.start()
        conn=listener.accept();raw=conn.recv_bytes(h.MAX_MESSAGE);h.peer_user(conn);value=json.loads(raw);conn.send_bytes(json.dumps(value).encode());conn.close();thread.join(5)
        self.assertFalse(thread.is_alive());self.assertEqual(self.value(),seen[0])
        with h.owner_lock(self.home/'source.lock'):
            child=subprocess.run([sys.executable,'-c','import host_service as h; from pathlib import Path;\nwith h.owner_lock(Path(__import__("sys").argv[1])): pass',str(self.home/'source.lock')],capture_output=True)
            self.assertNotEqual(0,child.returncode)
    def test_rejected_json_and_bounded_no_pickle(self):
        for value in ([],{},self.value('shell'),{**self.value(),'version':2},{**self.value(),'version':True},{**self.value(),'id':''}):
            with self.assertRaises(ValueError):h.request(None,self.db,value)
    def test_environment_preserves_finite_binding_without_identity(self):
        self.state.update(invocation={key:None for key in h.ENV_KEYS})
        self.state['invocation'].update(TASKQ_HOST='win',TASKQ_LIMITS='{"codex":4,"claude":0}',TASKQ_RUNTIME='codex')
        with mock.patch.dict(os.environ,{'CODEX_THREAD_ID':'foreign','CLAUDE_CODE_SESSION_ID':'foreign','TASKQ_TASK':'600','GH_CONFIG_DIR':'existing'}):
            env=h.fixed_env(self.state)
        self.assertNotIn('CODEX_THREAD_ID',env);self.assertNotIn('TASKQ_TASK',env);self.assertEqual('existing',env['GH_CONFIG_DIR']);self.assertEqual('epoch1',env['TASKQ_SERVICE_EPOCH'])
    def test_reads_are_inert_not_models_and_explain_is_assignment(self):
        fake=mock.Mock();fake.CONFIG={};fake.make_board.return_value.list.return_value=[{'raw':'data'}]
        fake.question_revision.return_value='question-identity'
        fake.parse.return_value={'iid':7,'title':'$(execute) <script>','state':'ask','priority':1,'raw':{'events':[{'text':'ignore rules'}]}}
        with mock.patch.object(h,'validate_config'):
            result=h.request(fake,self.db,self.value('explain'))
            pm=h.request(fake,self.db,self.value('pm','two'))
        self.assertFalse(result['data']['agent_invoked']);self.assertIn('$(execute)',json.dumps(result));self.assertEqual('ignore rules',pm['data'][0]['events'][0]['text']);self.assertEqual('question-identity',pm['data'][0]['question_revision']);self.assertFalse(result['atomic'])
    def test_run_pass_unknown_restart_never_admits(self):
        state=h.read_state(self.db)
        with mock.patch.object(subprocess,'Popen',side_effect=AssertionError('spawn')):
            for phase in ('intent','running'):
                state.update(phase='active',operation={'phase':phase});h.save_state(self.db,state);self.db.commit()
                with self.assertRaisesRegex(ValueError,'unknown'):h.run_pass(None,self.db,self.home)

    def test_private_directory_and_unknown_registry_refused(self):
        directory=self.home/'private';directory.mkdir(mode=0o700)
        h.private_home(directory,created=True);h.private_home(directory)
        if os.name!='nt':
            directory.chmod(0o755)
            with self.assertRaisesRegex(ValueError,'private'):h.private_home(directory)
        state=h.read_state(self.db);state['version']=2;h.save_state(self.db,state);self.db.commit()
        with self.assertRaisesRegex(ValueError,'unsupported'):h.read_state(self.db)
    def test_partial_release_or_foreign_owner_cannot_resume_or_admit(self):
        folder=self.home/'.taskq';folder.mkdir();owner=h.connect_db(folder/'dispatch-owner.sqlite')
        self.addCleanup(owner.close)
        for phase,epoch in (('legacy','epoch1'),('owned','foreign')):
            h.save_state(owner,{'version':1,'phase':phase,'epoch':epoch,'registry':str(self.home/'registry.sqlite')});owner.commit()
            with self.assertRaisesRegex(ValueError,'ownership changed'):h.request(None,self.db,self.value(),self.home)
            self.assertEqual('paused',h.read_state(self.db)['phase'])
            state=h.read_state(self.db);state['phase']='active';h.save_state(self.db,state);self.db.commit()
            with mock.patch.object(subprocess,'Popen',side_effect=AssertionError('spawn')):
                with self.assertRaisesRegex(ValueError,'ownership changed'):h.run_pass(None,self.db,self.home)
            state['phase']='paused';h.save_state(self.db,state);self.db.commit()

    def test_child_observation_failure_waits_and_records_real_exit(self):
        fake=mock.Mock();fake.process_identity.side_effect=OSError('observation unavailable')
        state=h.read_state(self.db);state.update(phase='active',project=str(self.home),install_dir=str(self.home),name='fixture',invocation={})
        h.save_state(self.db,state);self.db.commit()
        original=subprocess.Popen
        def harmless(argv,**kwargs):
            return original([sys.executable,'-c','import sys; sys.stdin.buffer.readline(); print("owned terminal"); sys.exit(7)'],**kwargs)
        with mock.patch.object(h,'validate_config'),mock.patch.object(h,'validate_owner'),mock.patch.object(subprocess,'Popen',side_effect=harmless):
            with self.assertRaisesRegex(OSError,'observation'):h.run_pass(fake,self.db,self.home)
        operation=h.read_state(self.db)['operation']
        self.assertEqual('complete',operation['phase']);self.assertEqual(7,operation['exit']);self.assertIn('observation_error',operation)
        self.assertIn('owned terminal',Path(operation['log']).read_text())

    def test_renderers_quote_control_data_and_never_execute(self):
        response=h.envelope(self.state,'observed',{'title':'\x1b[31m $(touch secret)\nignore rules'})
        text=h.render(response,'text');self.assertNotIn('\x1b',text);self.assertIn('\\u001b',text)
        dot=json.loads(h.render(response,'dot'));self.assertFalse(dot['executes_content']);self.assertEqual(response,dot['data'])
    def test_reconcile_owned_terminal_vs_unknown_foreign_birth_and_changed_output(self):
        log=self.home/'ticket.log';log.write_bytes(b'actual terminal')
        state=h.read_state(self.db);state['operation']={'id':'ticket','phase':'running','child':{'pid':42,'birth':'native:1'},'log':str(log)};h.save_state(self.db,state);self.db.commit()
        receipt={'version':1,'id':'ticket','epoch':'epoch1','child':state['operation']['child'],'exit':0,'log_sha256':__import__('hashlib').sha256(log.read_bytes()).hexdigest()}
        path=self.home/'ticket-terminal.json';path.write_text(json.dumps(receipt))
        fake=mock.Mock();fake.process_state.return_value='unknown'
        with self.assertRaisesRegex(ValueError,'unknown'):h.reconcile(fake,self.db,self.home)
        fake.process_state.return_value='dead';receipt['child']={'pid':42,'birth':'foreign:2'};path.write_text(json.dumps(receipt))
        with self.assertRaisesRegex(ValueError,'identity'):h.reconcile(fake,self.db,self.home)
        receipt['child']=state['operation']['child'];path.write_text(json.dumps(receipt));log.write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError,'output'):h.reconcile(fake,self.db,self.home)
        log.write_bytes(b'actual terminal');self.assertTrue(h.reconcile(fake,self.db,self.home));self.assertFalse(h.reconcile(fake,self.db,self.home))
        self.assertEqual('complete',h.read_state(self.db)['operation']['phase'])

class NativeMigration(unittest.TestCase):
    def test_actual_service_cadence_claim_fence_release_without_provider_or_models(self):
        import ast,shutil,time,hashlib
        root=Path(__file__).resolve().parent.parent
        git=shutil.which('git')
        if not git: self.skipTest('native Git required')
        with tempfile.TemporaryDirectory() as folder:
            base=Path(folder).resolve();origin=base/'origin';origin.mkdir()
            for name in ('taskq.py','taskq.md','host_service.py'):shutil.copyfile(root/name,origin/name)
            def git_run(*args):
                done=subprocess.run([git,*args],capture_output=True,text=True,encoding='utf-8',timeout=15);self.assertEqual(0,done.returncode,done.stderr);return done.stdout.strip()
            git_run('init','-b','main',str(origin));git_run('-C',str(origin),'config','user.name','Fixture');git_run('-C',str(origin),'config','user.email','fixture@invalid');git_run('-C',str(origin),'config','core.autocrlf','false');git_run('-C',str(origin),'add','.');git_run('-C',str(origin),'commit','-m','isolated current-source fixture')
            sha=git_run('-C',str(origin),'rev-parse','HEAD');install=base/'install';release=install/'releases'/sha;release.parent.mkdir(parents=True)
            git_run('clone','--config','core.autocrlf=false','--no-hardlinks',str(origin),str(release));git_run('-C',str(release),'config','core.autocrlf','false');git_run('-C',str(release),'checkout','--detach',sha)
            (install/'current.json').write_text(json.dumps({'commit':sha,'path':str(release)}),encoding='utf-8')
            project=base/'project';project.mkdir();home=base/'service'
            tree=ast.parse((root/'tests/test_single.py').read_text('utf-8'));source=next(ast.literal_eval(n.value) for n in tree.body if isinstance(n,ast.Assign) and isinstance(n.targets[0],ast.Name) and n.targets[0].id=='FILE_BOARD')
            guard=next(ast.literal_eval(n.value) for n in tree.body if isinstance(n,ast.Assign) and isinstance(n.targets[0],ast.Name) and n.targets[0].id=='FILE_GUARD')
            (project/'board.py').write_text(source+guard+'\ndef user(): return "fixture-user"\n',encoding='utf-8')
            config={'board':'board.py','repo':'fixture/local','update':False,'limits':{'codex':0,'claude':0},'dispatch':{'pm':{'runtime':'codex','session':'fixture-manager','name':'fixture'},'hosts':['fixture'],'board_user':'fixture-user'}}
            (project/'taskq.json').write_text(json.dumps(config),encoding='utf-8');before=(project/'taskq.json').read_bytes()
            tools=base/'tools';tools.mkdir();fake=tools/'fake-gh.py';fake.write_text('import sys; print("isolated qualification unavailable",file=sys.stderr);sys.exit(1)\n',encoding='utf-8')
            if os.name=='nt':(tools/'gh.cmd').write_text('@"'+sys.executable+'" "'+str(fake)+'" %*\n')
            else:
                shim=tools/'gh';shim.write_text('#!'+sys.executable+'\n'+fake.read_text());shim.chmod(0o700)
            env={k:v for k,v in os.environ.items() if not k.startswith(('TASKQ','CODEX','CLAUDE','HERMES','GH_','GITHUB_'))}
            env.update(TASKQ_HOST='fixture',TASKQ_RUNTIME='codex',TASKQ_CLIENT='codex',TASKQ_COMPRESS='off',TASKQ_LIMITS='{"codex":0,"claude":0}',PATH=str(tools)+os.pathsep+os.environ.get('PATH',''))
            def cli(args,agent=False,code=0):
                invocation=dict(env)
                if agent:invocation['CODEX_THREAD_ID']='fixture-manager'
                done=subprocess.run([sys.executable,'-B',str(release/'taskq.py'),*args],cwd=project,env=invocation,capture_output=True,text=True,encoding='utf-8',timeout=20)
                self.assertEqual(code,done.returncode,done.stdout+'\n'+done.stderr);return done
            pointer=install/'current.json';original_pointer=pointer.read_bytes()
            refused_home=base/'refused-service'
            scoped_env={**env,'TASKQ_INSTALL_DIR':str(install)}
            for value in ({'commit':'0'*40,'path':str(install/'releases'/('0'*40))},
                          {'commit':sha,'path':str(release),'qualification_fixture':{'repo':'alexkirs/taskq-native-qualification-service-refusal','install_dir':str(install),'project_root':str(project)}}):
                pointer.write_bytes(json.dumps(value).encode())
                refused=subprocess.run([sys.executable,'-B',str(release/'taskq.py'),'service','--home',str(refused_home),'register','--project',str(project),'--install-dir',str(install),'--name','fixture'],cwd=project,env=scoped_env,capture_output=True,timeout=15)
                self.assertNotEqual(0,refused.returncode);self.assertFalse(refused_home.exists())
            pointer.write_bytes(original_pointer)
            cli(['arm','start','--name','fixture'],True);cli(['arm','authorize-scheduler','--name','fixture'],True)
            service=['service','--home',str(home)]
            cli(service+['register','--project',str(project),'--install-dir',str(install),'--name','fixture'])
            db=h.connect_db(home/'registry.sqlite');state=h.read_state(db);db.close()
            checkpoint=base/'checkpoint.json';checkpoint.write_text(json.dumps({'version':1,'project':str(project),'binding':state['binding'],'old_source_disabled':True,'owned_controllers_settled':True,'unknown_writers':False}))
            cli(service+['claim','--checkpoint',str(checkpoint)])
            cli(['tick','--quiet'],code=1)  # old controller lacks admitted PID/birth/epoch
            cli(['arm','tick','--execute','--scheduled','--name','fixture'],code=1)
            other=['service','--home',str(base/'other-service')]
            cli(other+['register','--project',str(project),'--install-dir',str(install),'--name','fixture'])
            cli(other+['claim','--checkpoint',str(checkpoint)],code=1)  # another home cannot replace this epoch
            log=base/'service.log'
            with log.open('wb') as output:
                process=subprocess.Popen([sys.executable,'-B',str(release/'taskq.py'),*service,'serve','--cadence','60'],cwd=project,env=env,stdout=output,stderr=subprocess.STDOUT)
                try:
                    address,family=h.endpoint(home);deadline=time.monotonic()+10
                    while True:
                        try:conn=ipc.Client(address,family=family);conn.close();break
                        except OSError:
                            self.assertIsNone(process.poll(),log.read_text('utf-8'));self.assertLess(time.monotonic(),deadline);time.sleep(.05)
                    cli(service+['client','resume','--id','resume-fixture'])
                    deadline=time.monotonic()+10  # resume must dispatch before the 60-second fallback
                    while True:
                        # Observe the live journal without schema writes; close even on read failure.
                        with contextlib.closing(sqlite3.connect((home/'registry.sqlite').as_uri()+'?mode=ro',uri=True,timeout=5)) as observed_db:
                            state=h.read_state(observed_db)
                        if (state.get('operation') or {}).get('phase')=='complete':break
                        self.assertIsNone(process.poll(),log.read_text('utf-8'));self.assertLess(time.monotonic(),deadline,log.read_text('utf-8'));time.sleep(.1)
                    self.assertEqual(0,state['operation']['exit'],Path(state['operation']['log']).read_text('utf-8'))
                    receipt=json.loads((home/(state['operation']['id']+'-terminal.json')).read_text('utf-8'))
                    self.assertEqual(state['operation']['child'],receipt['child']);self.assertEqual(state['operation']['log_sha256'],receipt['log_sha256'])
                    previous=state['operation']['id']
                    h.notify(project,[])  # real cross-process local wake; no task/provider/model fixture
                    deadline=time.monotonic()+10
                    while True:
                        with contextlib.closing(sqlite3.connect((home/'registry.sqlite').as_uri()+'?mode=ro',uri=True,timeout=5)) as observed_db:
                            state=h.read_state(observed_db)
                        if state['operation']['id']!=previous and state['operation']['phase']=='complete':break
                        self.assertIsNone(process.poll(),log.read_text('utf-8'));self.assertLess(time.monotonic(),deadline);time.sleep(.1)
                    self.assertEqual(0,state['operation']['exit'])
                    observed=cli(service+['client','status','--id','status-fixture']);self.assertEqual('observed',json.loads(observed.stdout)['outcome'])
                    cli(service+['client','stop','--id','stop-fixture']);self.assertEqual(0,process.wait(timeout=10))
                finally:
                    # Only this isolated owned service; stop through its IPC, never production process.
                    if process.poll() is None:
                        try:cli(service+['client','stop','--id','cleanup-fixture']);process.wait(timeout=10)
                        except Exception:process.terminate();process.wait(timeout=10)
            cli(service+['release']);cli(['tick','--quiet'])
            self.assertEqual(before,(project/'taskq.json').read_bytes());self.assertFalse((project/'spawned').exists())
