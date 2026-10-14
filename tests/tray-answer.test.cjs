// Chain: tray shared card -> IPC -> canonical dispatcher receipt. Risk: a lost response changes operation identity,
// a foreign receipt accepts an answer, or stale project data grants controls. Native interaction is a separate gate.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const card=require('../src/answer-card.js');
const html=fs.readFileSync('src/tray.html','utf8');
const source=html.slice(html.indexOf('// Task answers:'),html.indexOf('// one task per board'));
const sent=[],timers=new Map();let id=0;
const board='https://github.com/o/r/issues',project={root:'project',issues:board,alive:true,complete:true};
const context={S:{projects:[project],font:16},send:m=>sent.push(m),fit:()=>{},render:()=>{},menu:{scrollHeight:400},esc:String,
  setTimeout:(f,ms)=>{timers.set(++id,{f,ms});return id;},clearTimeout:i=>timers.delete(i)};
vm.createContext(context);vm.runInContext(source,context);
const call=(name,...args)=>{context.args=args;return vm.runInContext(name+'(...args)',context);};
const plain=x=>JSON.parse(JSON.stringify(x));
(async()=>{
  assert.equal(call('answerCapability',board).answer,true);
  for(const state of ['doing','review','ready','later','waiting','blocked']) assert.equal(call('answerSlot',{state,key:'task',card:'Old question',question:{parsed:true}}),'',state+' must not mount an answer card');
  assert(call('answerSlot',{state:'ask',key:'task'}).includes('data-answer'));
  project.complete=false;assert.equal(call('answerCapability',board).answer,false);
  project.complete=true;project.alive=false;assert.equal(call('answerCapability',board).answer,false);project.alive=true;
  assert.equal(call('answerCapability','other-board').answer,false);
  const op={id:'operation-1',task:841,revision:4,text:'literal 2'};
  const first=call('answerSend',board,op);
  assert.equal(call('answerSend',board,op),first,'duplicate bridge request shares its promise');
  assert.deepEqual(plain(sent),[{a:'answer',root:'project',board,op}]);
  call('answerReceipt',{root:'other',id:op.id,state:'accepted'});
  call('answerReceipt',{root:'project',id:'other',state:'accepted'});
  assert.equal(timers.size,1,'foreign receipt leaves request pending');
  [...timers.values()][0].f();
  assert.equal((await first).state,'unknown','timeout never accepts');
  const retry=call('answerSend',board,op);
  assert.deepEqual(plain(sent[1].op),op,'reconciliation keeps exact op ID, revision and literal text');
  call('answerReceipt',{root:'project',id:op.id,state:'accepted',reason:'read back',next:''});
  assert.deepEqual(plain(await retry),{state:'accepted',reason:'read back',next:''});
  // A1/A2 #1024: a refresh past the op's question reconciles once with the same op; only its receipt decides the state.
  for(const [move,state] of [[{revision:5},'accepted'],[{state:'ready'},'stale']]){
    const o={...op,id:'readback-'+state},before=sent.length,pending=call('answerSend',board,o);
    project.tasks=[{n:841,state:'ask',revision:4}];call('answerReadback');
    assert.equal(sent.length,before+1,'unchanged question: no reconcile');
    project.tasks=[{n:841,state:'ask',revision:4,...move}];call('answerReadback');call('answerReadback');
    assert.deepEqual(plain(sent.slice(before+1)),[{a:'answer',root:'project',board,op:o}],'one same-op reconcile');
    call('answerReceipt',{root:'project',id:o.id,state});
    assert.equal((await pending).state,state,'refresh alone never accepts');
  }
  const act={...op,id:'readback-later',action:'later'},before=sent.length;call('answerSend',board,act);
  project.tasks=[{n:841,state:'later',revision:5}];call('answerReadback');
  assert.equal(sent.length,before+1,'fallback action keeps Check delivery, no readback reconcile');
  call('answerReceipt',{root:'project',id:act.id,state:'accepted'});
  delete project.tasks;
  context.S.projects=[];
  assert.equal((await call('answerSend',board,op)).state,'rejected');
  assert.equal(sent.length,7,'unregistered board writes nothing');
  // Run the consumer's capability across shared stale/lost-response fixtures; component owns draft and op state.
  const fx=JSON.parse(fs.readFileSync('tests/fixtures/answer-card.json','utf8'));
  for(const c of fx.cases.filter(c=>/stale|lost|duplicate|sending/.test(c.name))){
    context.S.projects=[{...project,issues:c.board}];
    const got=card.replay(c,(b,r)=>r.capability||c.capability&&{...c.capability,answer:c.capability.answer&&call('answerCapability',b).answer});
    c.events.filter(e=>e.type==='check').forEach((want,i)=>{
      if(want.status)assert.equal(got[i].status,want.status,c.name);
      if(want.draft!=null)assert.equal(got[i].draft,want.draft,c.name);
      if(want.same_op)assert.deepEqual(got[i].sent.at(-1),got[i].sent.at(-2),c.name);
    });
  }
  // A2: the tray alone grants fallback actions; every transition keeps revision, confirmation and retry identity.
  const row={n:42,state:'ask',revision:12,card:'Reviewer needs help.',question:{parsed:false,text:'Reviewer needs help.',choices:[]}},cap={answer:true,manage:true};
  const fresh=()=>({draft:'',bound:12,op:null,receipt:null});
  let local=fresh(),seq=0;
  const step=ev=>{const r=card.step(row,local,cap,ev,()=> 'fallback-'+ ++seq);local=r.local;return r.send;};
  assert.equal(card.status(row,local,{answer:true}),'read-only','other consumers gain no fallback authority');
  assert(card.render(board,row,local,cap).includes('Requeue'));
  assert.equal(step({type:'submit'}),null,'requeue needs a reason');
  step({type:'input',text:'Please retry'});
  const rq=step({type:'submit'});assert.deepEqual(rq,{id:'fallback-1',task:42,revision:12,action:'requeue',text:'Please retry'});
  assert.equal(step({type:'later'}),null,'sending disables a second action');
  step({type:'receipt',opId:rq.id,state:'unknown'});assert.deepEqual(step({type:'retry'}),rq);
  local=fresh();assert.equal(step({type:'confirm-close'}),null,'cannot skip confirmation');
  assert.equal(step({type:'close'}),null);assert(card.render(board,row,local,cap).includes('without publication'));
  assert.equal(step({type:'cancel-close'}),null);assert.equal(step({type:'confirm-close'}),null);
  step({type:'close'});const close=step({type:'confirm-close'});assert.equal(close.action,'close');assert.equal(close.confirmed,true);
  local=fresh();assert.equal(step({type:'later'}).action,'later');
  local=fresh();step({type:'close'});row.revision=13;assert.equal(step({type:'confirm-close'}),null,'confirmation is bound to its question');row.revision=12;
  local={...fresh(),draft:'old draft'};row.revision=13;
  assert.equal(step({type:'later'}),null,'stale fallback cannot cancel or park');
  assert.equal(card.status(row,local,cap),'stale');
  step({type:'review'});assert.equal(step({type:'later'}).revision,13);
  for(const state of ['doing','later','review']) assert.equal(card.status({...row,state},fresh(),cap),'read-only');
  assert(html.includes('<script>{{ANSWER_CARD_JS}}</script>'));
  assert(html.includes('<style>{{ANSWER_CARD_CSS}}</style>'));
  assert(html.includes("if(s.answerReceipt){answerReceipt(s.answerReceipt);return;}"));
  console.log('PASS tray capability, exact IPC payload, duplicate request, foreign receipt, timeout, same-ID retry, shared stale/lost-response fixtures');
})().catch(e=>{console.error(e);process.exit(1);});
