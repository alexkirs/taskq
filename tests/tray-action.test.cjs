// Chain: command failure receipt -> visible escaped reason -> retry, without clearing another project's pending action.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const html=fs.readFileSync('src/tray.html','utf8');
let renders=0;
const context={S:{projects:[{root:'one',name:'<project>'},{root:'two',name:'second'}]},pending:{one:'Resuming',two:'Pausing'},render:()=>renders++,esc:s=>String(s).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('"','&quot;')};
vm.createContext(context);
vm.runInContext(html.slice(html.indexOf('const actionErrors='),html.indexOf('\n',html.indexOf('function actionNotices('))),context);
vm.runInContext("actionReceipt({root:'one',text:'no saved focus <retry>'})",context);
assert.equal(context.pending.one,undefined);assert.equal(context.pending.two,'Pausing');assert.equal(renders,1);
const notice=vm.runInContext('actionNotices()',context);
assert(notice.includes('role="alert"'));assert(notice.includes('&lt;project>'));assert(notice.includes('&lt;retry>'));
vm.runInContext("actionReceipt({root:'unregistered',text:'invalid'})",context);assert.equal(renders,1);
assert.equal(vm.runInContext('actionNotices()',context),notice);
vm.runInContext("actionErrors.delete('one')",context);assert.equal(vm.runInContext('actionNotices()',context),'');
assert(html.includes('if(s.action_error){actionReceipt(s.action_error);return;}'));
console.log('PASS action refusal, pending isolation, visible reason and retry');
