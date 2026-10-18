// Load before product modules: a broken renderer must not keep apparently usable controls.
function install(window=globalThis){
const document=window.document;
const counts=[0,0,0,0],classes=['script','rejection','resource','evaluation'];
let dirty=false,timer=0,shown=false,saturated=false;
function notice(){
  if(shown||!document.body)return;shown=true;
  const style=document.createElement('style');style.textContent='.menu{display:none!important}';document.head.append(style);
  const box=document.createElement('div');box.setAttribute('role','alert');
  box.style.cssText='box-sizing:border-box;padding:12px;max-width:100%;overflow-wrap:anywhere';
  box.textContent='Interface error; command outcomes unknown. Close and reopen TaskQ. Check service status before retrying a command.';
  document.body.append(box);
}
function flush(){
  timer=0;if(!dirty)return;dirty=false;
  try{window.ipc.postMessage(JSON.stringify({a:'web-error',counts:counts.slice(),saturated}));}catch(_){/* native bootstrap/evaluation deadlines expose a missing bridge */}
  timer=window.setTimeout(flush,2000);
}
window.taskqWebFault=kind=>{
  const i=classes.indexOf(kind);if(i>=0){if(counts[i]<4294967295)counts[i]++;else saturated=true;dirty=true;}
  notice();if(dirty&&!timer)flush();
};
window.addEventListener('DOMContentLoaded',()=>{if(counts.some(Boolean))notice();});
window.addEventListener('error',e=>window.taskqWebFault(e.target===window?'script':'resource'),true);
window.addEventListener('unhandledrejection',()=>window.taskqWebFault('rejection'));
}
module.exports={install};
