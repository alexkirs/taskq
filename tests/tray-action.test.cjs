// Chain: click -> native receipt -> exact fresh service observation; no mode-change or stale-reply acknowledgment.
const fs=require('node:fs'),assert=require('node:assert/strict');
const timers=[],sent=[];
let renders=0;
const context={S:{projects:[{root:'one',name:'<project>',focus:[],saved:true,age:0},{root:'two',name:'second',focus:[],age:0}]},pending:{},
  render:()=>renders++,send:m=>sent.push(m),setTimeout:(f,ms)=>{assert.equal(ms,30000);timers.push(f);},crypto:require('node:crypto').webcrypto,
  esc:s=>String(s).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('"','&quot;'),window:{},settle(){},answerReadback(){},aiText:''};
require('./tray-harness.cjs').install(context);
const receipt='a'.repeat(32),other='b'.repeat(32),observed='d'.repeat(32);
(context.focusStart('resume','one'),context.focusStart('pause','two'),context.focusStart('resume','one'));
assert.equal(sent.length,2,'a waiting click must not dispatch twice');
const first=sent[0].id,second=sent[1].id;
assert.match(first,/^[a-f0-9]{32}$/);assert.notEqual(first,second);
// Another command changes the visible mode; pending still belongs to the original click.
context.receive({projects:[{root:'one',name:'<project>',focus:['all'],age:0,focus_command:{state:'observed',applied_receipt:other}},context.S.projects[1]]});
assert.equal(context.pending.one.id,first);
context.receive({focus_receipt:{root:'one',id:first,applied_receipt:receipt}});
assert.equal(context.pending.one.text,'Waiting for service');
for(const age of [null,-1,61]){
  context.S.projects[0].focus_command={state:'observed',applied_receipt:receipt,observed_receipt:observed};context.S.projects[0].age=age;
  (context.focusReadback());assert(context.pending.one,'stale/unknown age cannot acknowledge');
}
context.S.projects[0].age=0;context.S.projects[0].focus_command.state='unknown';
(context.focusReadback());assert(context.pending.one);
context.S.projects[0].focus_command.state='observed';context.S.projects[0].focus_command.observed_receipt=null;
(context.focusReadback());assert(context.pending.one,'missing observed receipt cannot confirm');
context.S.projects[0].focus_command.observed_receipt=observed;
// Deadline does not retry or invent failure; a late matching observation resolves the unknown.
timers[0]();assert(context.pending.one.unknown);assert((context.actionNotices()).includes('confirmation unknown'));
assert.equal(sent.length,2);
context.S.projects[0].focus_command.state='observed';(context.focusReadback());
assert.equal(context.pending.one,undefined);assert.equal((context.actionNotices()),'');
assert.equal(context.pending.two.id,second);
// A snapshot may arrive before the native reply; unchanged mode still confirms the exact command.
(context.focusStart('resume','one'));const next=sent.at(-1).id;
context.receive({focus_receipt:{root:'one',id:first,text:'late failure'}});
assert.equal(context.pending.one.id,next);assert.equal((context.actionNotices()),'');
context.S.projects[0].focus_command={state:'observed',applied_receipt:'c'.repeat(32),observed_receipt:'e'.repeat(32)};
(context.focusReadback());assert(context.pending.one,'snapshot without native reply cannot name this click');
context.receive({focus_receipt:{root:'one',id:next,applied_receipt:'c'.repeat(32)}});
assert.equal(context.pending.one,undefined,'matching pre-arrived snapshot did not clear identical-focus command');
// New explicit action after an unknown replaces only its page correlation, never retries old work.
(context.focusStart('pause','one'));const expired=sent.at(-1).id;timers.at(-1)();
(context.focusStart('resume','one'));const latest=sent.at(-1).id;assert.notEqual(latest,expired);
context.receive({focus_receipt:{root:'one',id:expired,applied_receipt:receipt}});
assert.equal(context.pending.one.id,latest);
context.receive({focus_receipt:{root:'one',id:latest,text:'partially applied <inspect>'}});
assert.equal(context.pending.one,undefined);assert.equal(context.pending.two.id,second);
assert((context.actionNotices()).includes('&lt;project&gt;'));assert((context.actionNotices()).includes('&lt;inspect&gt;'));
// Non-focus failures cannot clear another outstanding command or disappear on its readback.
(context.actionReceipt({root:'two',text:'restart failed'}));assert.equal(context.pending.two.id,second);
context.S.projects[1].focus_command={state:'observed',applied_receipt:other,observed_receipt:observed};
context.receive({focus_receipt:{root:'two',id:second,applied_receipt:other}});
assert.equal(context.pending.two,undefined);assert((context.actionNotices()).includes('restart failed'));
const before=renders;(context.actionReceipt({root:'unregistered',text:'ignored'}));assert.equal(renders,before);
console.log('PASS exact fresh focus acknowledgment, stale reply isolation, unknown deadline, identical focus and escaped failures');
// H-A5: global review -> immutable restart identity -> original observation from a different generation.
context.S.projects[0].root='/canonical/one';context.S.projects[1].root='/canonical/two';
context.S.restart_review={id:'review',instance:'origin',roots:['/canonical/one','/canonical/two'],issued_at:1000,expires_at:1300};
context.S.restart={state:'unknown'};
assert.match(context.restartControl(context.S.projects[0]),/all registered projects/);
assert.match(context.restartControl(context.S.projects[0]),/second/);
assert.match(context.restartControl(context.S.projects[0]),/&lt;project&gt;/);
context.restartStart('/canonical/one');const restart=sent.at(-1),item=context.hostRestart;
assert.equal(restart.a,'restart');assert.equal(restart.instance,'origin');assert.deepEqual(restart.roots,['/canonical/one','/canonical/two']);
const sends=sent.length;context.restartStart('/canonical/one');assert.equal(sent.length,sends,'pending restart cannot dispatch twice');
context.restartReceipt({id:'another',state:'observed'});assert.equal(context.hostRestart.state,'sending');
context.receive({capmsg:{id:restart.id,root:'/canonical/one',state:'queued',received_receipt:receipt}});assert.equal(item.state,'queued');
context.S.restart={id:restart.id,state:'observed',received_receipt:receipt,applied_receipt:other,observed_receipt:observed,service_instance:'origin'};
context.restartReadback();assert.notEqual(item.state,'observed','the original generation cannot confirm a restart');
context.S.restart.service_instance='replacement';context.S.restart.applied_receipt=[other];context.restartReadback();assert.notEqual(item.state,'observed','wrong receipt type cannot confirm');
context.S.restart.applied_receipt=other;timers.at(-1)();assert.equal(item.state,'unknown');assert.equal(sent.length,sends,'deadline cannot repeat the native operation');
context.restartReadback();assert.equal(item.state,'observed');assert.equal(item.unknown,false);
context.restartStart('/canonical/one');const nextRestart=sent.at(-1);assert.notEqual(nextRestart.id,restart.id);
context.restartReceipt({...context.S.restart,id:restart.id});assert.equal(context.hostRestart.id,nextRestart.id);assert.equal(context.hostRestart.state,'sending');
context.restartReceipt({id:nextRestart.id,state:'observed'});assert.equal(context.hostRestart.state,'unknown','incomplete native result stays unknown');
context.S.restart_review={state:'unknown',reason:'host <unknown>'};const beforeRestart=sent.length;context.restartStart('/canonical/one');assert.equal(sent.length,beforeRestart);
assert.match(context.actionNotices(),/host &lt;unknown&gt;/);
console.log('PASS host restart scope, immutable request, different-generation exact receipt, late/error isolation and no deadline replay');
context.S.service={state:'running',pid:731,instance:'origin'};
Object.assign(context.S.projects[0],{service_pid:731,host_instance:'origin',age:0,restart_waits:['#7','operator']});
Object.assign(context.S.projects[1],{service_pid:731,host_instance:'old',age:0,restart_waits:['#99']});
assert.match(context.restartWaits(),/waits for #7, operator/);assert(!context.restartWaits().includes('#99'),'another generation cannot explain current readiness');
context.S.restart={id:context.hostRestart.id,state:'applied',reason:'required restart receipt unavailable: store-full'};
context.restartReadback();assert.match(context.hostRestart.reason,/store-full/);
console.log('PASS native-bound project waits and durable receipt failures remain visible');
