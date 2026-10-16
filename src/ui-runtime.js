// UI execution uses the existing native capture writer; no collector or storage here.
'use strict';
const core=require('@taskq/coreq');
const execution=core.shared(),events=new WeakMap();
const frequent=new Set(['scroll','pointermove','input']);
function send(message){
  const context=execution.context();
  globalThis.ipc?.postMessage(JSON.stringify({...message,...(context?{context:'1 '+context.trace+' '+context.span}:{})}));
}
function connect(){
  core.connect(row=>{
    if(!globalThis.ipc)return false;
    globalThis.ipc.postMessage(JSON.stringify({a:'web-observation',row}));return true;
  });
}
function listen(target,type,callback,options){
  const method=execution.module('ui.events',{[type]:callback})[type];
  const handler=execution.action('ui.'+type,function(event){
    if(event&&typeof event==='object')events.set(event,execution.bind(fn=>fn()));
    return Reflect.apply(method,this,[event]);
  });
  const receive=function(event){
    const inherited=event&&events.get(event);
    return inherited?inherited(()=>Reflect.apply(method,this,[event])):frequent.has(type)?Reflect.apply(method,this,[event]):Reflect.apply(handler,this,[event]);
  };
  target.addEventListener(type,receive,options);
  return ()=>target.removeEventListener(type,receive,options);
}
function defer(callback,delay=0){return setTimeout(execution.bind(execution.action('ui.timer',callback)),delay);}
function cancel(timer){clearTimeout(timer);}
function effect(name,start,receive){
  execution.effect(name,()=>Promise.resolve().then(execution.bind(start)),result=>receive(result.ok?result.value:{state:'unknown',reason:result.unknown?'Completion unknown':String(result.value?.message||result.value)}));
}
module.exports={execution,connect,send,listen,defer,cancel,effect};
