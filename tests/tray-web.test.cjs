// Chain: early page faults -> bounded safe IPC -> independent visible failure, even when the bridge fails.
const fs=require('node:fs'),assert=require('node:assert/strict');
const {install}=require('../src/tray-web.js');
function page(broken=false){
 const handlers={},timers=[],sent=[],nodes=[];
 const document={readyState:'complete',head:{append:x=>nodes.push(x)},body:{append:x=>nodes.push(x)},
  createElement:tag=>({tag,style:{},setAttribute(k,v){this[k]=v;}})};
 const context={document,setTimeout:(f,ms)=>{assert.equal(ms,2000);timers.push(f);return 1;},addEventListener:(n,f)=>handlers[n]=f};
 context.window=context;context.ipc={postMessage:m=>{if(broken)throw Error('secret');sent.push(JSON.parse(m));}};
 install(context);return {handlers,timers,sent,nodes,context,win:context};
}
const p=page();p.handlers.error({target:p.win,message:'SECRET',filename:'/private/secret',error:Error('SECRET')});
assert.equal(p.sent.length,1);assert.deepEqual(p.sent[0],{a:'web-error',counts:[1,0,0,0],saturated:false});
for(let i=0;i<100;i++)p.handlers.error({target:p.win});
p.handlers.unhandledrejection({reason:'SECRET'});p.handlers.error({target:{src:'SECRET'}});p.context.taskqWebFault('evaluation');
assert.equal(p.sent.length,1);assert.equal(p.timers.length,1);
p.timers.shift()();assert.deepEqual(p.sent.at(-1).counts,[101,1,1,1]);
assert(!JSON.stringify(p.sent).includes('SECRET'));assert.equal(p.nodes.filter(x=>x.role==='alert').length,1);
assert(p.nodes.some(x=>x.textContent.includes('.menu')));assert(p.nodes.some(x=>x.textContent.includes('outcomes unknown')));
p.timers.shift()();assert.equal(p.timers.length,0,'idle observer keeps no recurring timer');
const b=page(true);assert.doesNotThrow(()=>b.handlers.error({target:b.win}));assert(b.nodes.some(x=>x.role==='alert'));
assert.equal(b.timers.length,1,'broken reporter did not recurse');
console.log('PASS early page failure, bounded counters, no raw content, independent notice and broken bridge');
