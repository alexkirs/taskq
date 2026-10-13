// Chain: tray shared card -> IPC -> canonical dispatcher receipt. Risk: a lost response changes operation identity,
// a foreign receipt accepts an answer, or stale project data grants controls. Native interaction is a separate gate.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const card=require('../src/answer-card.js');
const html=fs.readFileSync('src/tray.html','utf8');
const source=html.slice(html.indexOf('// Task answers:'),html.indexOf('// one task per board'));
const sent=[],timers=new Map();let id=0;
const board='https://github.com/o/r/issues',project={root:'project',issues:board,alive:true,complete:true};
const context={S:{projects:[project],font:16},send:m=>sent.push(m),menu:{scrollHeight:400},
  setTimeout:(f,ms)=>{timers.set(++id,{f,ms});return id;},clearTimeout:i=>timers.delete(i)};
vm.createContext(context);vm.runInContext(source,context);
const call=(name,...args)=>{context.args=args;return vm.runInContext(name+'(...args)',context);};
const plain=x=>JSON.parse(JSON.stringify(x));
(async()=>{
  assert.equal(call('answerCapability',board).answer,true);
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
  context.S.projects=[];
  assert.equal((await call('answerSend',board,op)).state,'rejected');
  assert.equal(sent.length,2,'unregistered board writes nothing');
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
  assert(html.includes('<script>{{ANSWER_CARD_JS}}</script>'));
  assert(html.includes('<style>{{ANSWER_CARD_CSS}}</style>'));
  assert(html.includes("if(s.answerReceipt){answerReceipt(s.answerReceipt);return;}"));
  console.log('PASS tray capability, exact IPC payload, duplicate request, foreign receipt, timeout, same-ID retry, shared stale/lost-response fixtures');
})().catch(e=>{console.error(e);process.exit(1);});
