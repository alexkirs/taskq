// Chain: tray Capacity -> shared resource controls -> `service capacity` IPC receipt. Risk: an operation writes
// untouched fields, a foreign or lost receipt reads as saved, or an invalid config stays editable.
const fs=require('node:fs'),assert=require('node:assert/strict');
const rc=require('../src/resource-controls.js');
const fx=JSON.parse(fs.readFileSync('tests/fixtures/resource-controls.json','utf8'));
const sent=[],timers=new Map();let id=0;
const saved={limits:{claude:2},load_gate:{enabled:true,cpu_max:85,memory_min:10}};
const project={root:'p',name:'taskq',cap_host:'mac',cap_saved:saved,resources:fx.cases[0].groups[0]};
const context={S:{projects:[project],font:16,now:1000},TaskqResources:rc,esc:s=>String(s),send:m=>sent.push(m),fit:()=>{},menu:{scrollHeight:400},
  setTimeout:(f,ms)=>{timers.set(++id,{f,ms});return id;},clearTimeout:i=>timers.delete(i)};
require('./tray-harness.cjs').install(context);
const call=(name,...args)=>context[name](...args);
const plain=x=>JSON.parse(JSON.stringify(x));
(async()=>{
  const g=call('resourceGroup',project);
  assert.equal(g.key,'p');assert.deepEqual(plain(g.labels),{host:'mac',project:'taskq'});
  assert.equal(call('resourceCapability',g).edit,true);
  project.cap_saved={error:'bad toml'};
  assert.deepEqual(plain(call('resourceCapability',g)),{edit:false,reason:'Read-only here',note:'Config: bad toml'});
  project.cap_saved=saved;
  assert(call('capacity',project).includes('Legacy guard · Fixed limits · Load gate: Ready'));
  assert.equal(call('capacity',{...project,resources:null}),'','no projection, no section');
  // the lost-receipt fixture's operation maps onto exactly its changed fields
  const lost=fx.cases.find(c=>/lost receipt/.test(c.name)),op={id:'op-1',ops:lost.events.find(e=>e.ops).ops};
  const first=call('resourceSend',g,op);
  assert.deepEqual(plain(sent),[{a:'capacity',root:'p',id:'op-1',set:{limits:{codex:1}},expect:saved}]);
  call('resourceReceipt',{root:'other',id:'op-1',text:'Saved: x'});
  call('resourceReceipt',{root:'p',id:'op-2',text:'Saved: x'});
  assert.equal(timers.size,1,'foreign receipt leaves request pending');
  [...timers.values()][0].f();
  assert.equal((await first).state,'unknown','timeout never reads as saved');
  const original=plain(sent[0]);
  saved.limits.claude=9;
  project.cap_saved={...saved,limits:{claude:4}};
  op.ops[0].changes.runtime_limit=7;
  const again=call('resourceSend',g,op);
  assert.deepEqual(plain(sent[1]),original,'retry freezes the entire first payload despite config and operation changes');
  call('resourceReceipt',{root:'p',id:'op-1',text:'Error: config changed on disk'});
  assert.deepEqual(plain(await again),{state:'rejected',reason:'config changed on disk'});
  const gate=fx.cases.find(c=>/saved then applied/.test(c.name)).events.find(e=>e.ops).ops;
  assert.deepEqual(plain(call('resourceSet',{ops:gate})),{limits:{},enabled:true,cpu_max:70,memory_min:10});
  const done=call('resourceSend',g,{id:'op-3',ops:gate});
  call('resourceReceipt',{root:'p',id:'op-3',text:'Saved: load gate'});
  assert.equal((await done).state,'saved');
  // #954: Auto/maximum travel only when edited; null removes the maximum, never zero
  assert.deepEqual(plain(call('resourceSet',{ops:[{target:{},changes:{mode:'fixed'}}]})),{limits:{},mode:'fixed'});
  assert.deepEqual(plain(call('resourceSet',{ops:[{target:{},changes:{maximum:null}}]})),{limits:{},maximum:null});
  assert(call('workEntry','p').includes('data-root="p"'),'project entry preselects that project');
  assert(call('workEntry').includes('data-a="work"')&&!call('workEntry').includes('data-root'),'home entry selects nothing');
  const host=structuredClone(project.resources),h=host.targets[0],p=host.targets[1];
  for(const t of host.targets)t.supported=[{policy_schema:'taskq.host-capacity/1',modes:[],fields:[],scopes:[]}];
  h.current_policy={maximum:null,load_gate:saved.load_gate};h.effective_policy=h.current_policy;
  h.runtime_usage=[{runtime_id:'codex',used:2,limit:3,limit_kind:'ceiling'}];h.sample=p.sample;h.blockers=[];
  p.current_policy={limits:{codex:1}};p.effective_policy=p.current_policy;
  project.resources=host;
  const hg=call('resourceGroup',project),cap=call('resourceCapability',hg);
  assert.equal(cap.edit,false,'legacy writer is not a host editor');
  assert.equal(rc.fields(hg).length,0);
  const before=sent.length;
  assert.equal((await call('resourceSend',g,{id:'stale-project-op',ops:gate})).state,'rejected');
  assert.equal(sent.length,before,'a stale project editor cannot send a host edit');
  const html=rc.render(hg,{draft:null,op:null,receipt:null},cap,1000);
  assert(html.includes('Host limits · no maximum')&&html.includes('2/3'));
  assert(html.includes('Project ceilings: Codex 1'));
  assert(!html.includes('Auto')&&!html.includes('Fixed')&&!html.includes('Edit limits'));
  assert(html.includes('Host limits affect all registered projects'));
  assert.equal(rc.gate(hg,1100).startsWith('Unknown:'),true,'stale host observation cannot look ready');
  console.log('PASS host observation, separate project ceiling and read-only cutover; legacy capacity IPC/receipts');
})().catch(e=>{console.error(e);process.exit(1);});
