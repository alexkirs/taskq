// Risk: future files/handlers use ordinary JS syntax that silently escapes the shared execution contract.
const assert=require('node:assert/strict'),inventory=require('../scripts/check-web.cjs');
const operations=inventory("const module=execution.module('new-file',{added(){return 1;}});ui.listen(node,'wheel',()=>module.added());for(const event of ['focusin','focusout'])ui.listen(node,event,()=>{});",'new-file.js');
for(const name of ['js.new-file.added','js.ui.events.wheel','js.ui.events.focusin','js.ui.events.focusout'])assert(operations.has(name),name);
for(const source of ['class NewModule { added(){} }','const module={added(){return 1;}};','const helper=()=>1;','function helper(){}','setTimeout(()=>{},1);',"addEventListener('click',()=>{});",'window.setInterval(()=>{},1);','const module=execution.module(\'m\',{async detached(){}});'])
 assert.throws(()=>inventory(source,'new-file.js'),/execution\.module|execution\.effect|execution boundary/,source);
console.log('PASS new file/method/event inventory and rejected unowned method/timer boundaries');
