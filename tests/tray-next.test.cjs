// Chain: § 4 tray Next buttons (R6 item 7) in src/tray.html. Risk: buttons out of order, label differs from the text, wrong reply token, buttons from a malformed line.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const html=fs.readFileSync('src/tray.html','utf8');
const source=html.slice(html.indexOf('const aiStates='),html.indexOf('// Add project'));
const req=text=>({id:'7',root:'p',text:'What next?',answer:{state:'result',text}});
const context={S:{projects:[],ai:{target:'p',history:[],request:req('Two tasks wait.\nNext: 1 Answer #41 ★ · 2 Start #44')}},esc:s=>String(s).replaceAll('<','&lt;'),aiText:'',aiMenu:false,aiConfirm:null};
vm.createContext(context);vm.runInContext(source,context);
let h=vm.runInContext('ai()',context);
const b=[...h.matchAll(/<button data-a="ai-next" data-v="(\d+)"( class="rec")?>([^<]*)<\/button>/g)];
assert.deepEqual(b.map(m=>[m[1],!!m[2],m[3]]),[['1',true,'1 Answer #41 ★'],['2',false,'2 Start #44']]);
assert(h.includes('Next: 1 Answer #41 ★ · 2 Start #44'));
// the click sends the number as a reply to this request, the same as typing it
const sent=[];context.send=m=>sent.push(m);context.render=()=>{};
vm.runInContext(html.slice(html.indexOf("  if(d.a==='ai-next')"),html.indexOf("  if(d.a==='ai-send')")).replace('if(','function click(d){if(')+'}',context);
vm.runInContext("click({a:'ai-next',v:'2'})",context);
assert.equal(JSON.stringify(sent),JSON.stringify([{a:'ai-send',text:'2',after:'7'}]));
for(const t of ['No next step.','Next: 2 Start ★ · 1 Answer','Next: 1 A ★ · 2 B ★','Next: Answer #41','Next: 1 A ★ · 2 B\nmore text']){
  context.S.ai.request=req(t);h=vm.runInContext('ai()',context);
  assert(!h.includes('data-a="ai-next"'),t);assert(h.includes(t.split('\n')[0].replaceAll('<','&lt;')),t);
}
context.S.ai.request={...req('Next: 1 A'),answer:{state:'running',text:'Next: 1 A'}};
assert(!vm.runInContext('ai()',context).includes('data-a="ai-next"'));
console.log('PASS Next buttons in order with ★, labels equal text, number reply, none for absent or malformed lines');
