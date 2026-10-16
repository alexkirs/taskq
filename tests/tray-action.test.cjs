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
