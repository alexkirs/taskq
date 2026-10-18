// Risk: a new handler/effect bypasses capture, a late result inherits another click, or frequent input floods IPC.
const assert=require('node:assert/strict');
const timers=new Map(),messages=[];let serial=0;
global.setTimeout=(fn,delay)=>{const id=++serial;timers.set(id,{fn,delay});return id;};global.clearTimeout=id=>timers.delete(id);
global.ipc={postMessage:text=>messages.push(JSON.parse(text))};
const ui=require('../src/ui-runtime.js');ui.connect();
function surface(){const handlers={};return {addEventListener:(type,fn)=>(handlers[type]??=[]).push(fn),removeEventListener(){},fire(type,event={}){for(const fn of handlers[type])fn(event);}};}
(async()=>{
 const target=surface();let origin,delivered,resolve;
 const pending=new Promise(r=>resolve=r);
 ui.listen(target,'click',()=>{
  origin=ui.execution.context();
  ui.effect('fixture.effect',()=>{ui.send({a:'fixture',text:'private product payload'});return pending;},value=>{delivered={value,context:ui.execution.context()};});
 });
 ui.listen(target,'click',()=>assert.equal(ui.execution.context().trace,origin.trace,'bubbling handlers lost their event'));
 target.fire('click');
 ui.execution.action('unrelated',()=>{})();
 await Promise.resolve();
 const command=messages.find(m=>m.a==='fixture');assert(command.context.startsWith('1 '+origin.trace+' '),'deferred adapter lost initiating trace');
 resolve('private result');for(let i=0;i<6;i++)await Promise.resolve();
 assert.equal(delivered.value,'private result');assert.equal(delivered.context.trace,origin.trace);assert.equal(ui.execution.context(),null);
 const secret={message:'PRIVATE ERROR'};ui.listen(target,'change',()=>{throw secret;});assert.throws(()=>target.fire('change'),e=>e===secret);
 const added=ui.execution.module('fixture-new-module',{newMethod(x){return x+1;}});assert.equal(added.newMethod(2),3);
 ui.execution.diagnose('js.fixture-new-module.',60000,2);added.newMethod(3);added.newMethod(4);added.newMethod(5);
 const before=messages.length;let calls=0;ui.listen(target,'pointermove',()=>{calls++;});
 const start=performance.now();for(let i=0;i<10000;i++)target.fire('pointermove');const elapsed=performance.now()-start;
 assert.equal(calls,10000);assert.equal(messages.length,before,'frequent events emitted individual detail');
 assert.equal([...timers.values()].filter(t=>t.delay===2000).length,1,'counter flushes did not coalesce');
 [...timers.values()].find(t=>t.delay===2000).fn();
 const rows=messages.filter(m=>m.a==='web-observation').map(m=>m.row);
 assert(rows.some(r=>r.operation==='js.ui.events.change'&&r.counts?.failed===1));
 assert(rows.some(r=>r.operation==='js.fixture-new-module.newMethod'&&r.counts?.calls===4));
 assert.equal(rows.filter(r=>r.operation==='js.fixture-new-module.newMethod'&&r.kind==='method').length,2);
 assert(rows.some(r=>r.operation==='js.ui.events.pointermove'&&r.counts?.calls===10000));
 assert(!/PRIVATE|private product|private result/.test(JSON.stringify(rows)),'telemetry leaked arguments/results/errors');
 assert.equal(ui.execution.health().lost,0);assert.equal(ui.execution.health().pending,0);
 console.log('PASS new handlers/methods, context through deferred adapter and late completion, throw identity, bounded detail and 10000 coalesced events ('+elapsed.toFixed(2)+'ms on this Node runner)');
})().catch(e=>{console.error(e);process.exitCode=1;});
