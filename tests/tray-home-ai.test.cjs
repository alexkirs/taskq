// Chain: #1059/#1115 Home inline AI exchange in src/tray.html. Risk: a long answer or proposal fills Home and pushes Projects off it, or a bounded preview swallows reply and confirm controls.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const html=fs.readFileSync('src/tray.html','utf8');
const source=html.slice(html.indexOf('const aiStates='),html.indexOf('// Add project'));
// both previews are bounded, scroll inside themselves and end on a whole line
assert.match(html,/\.aichain\{max-height:10em;overflow-y:auto[;}]/);
assert.match(html,/\.aiops\{max-height:6\.5em;overflow-y:auto[;}]/);
assert.match(html,/querySelectorAll\('\[data-bound\]'\)\.forEach\(c=>\{[^}]*wholeLines\(c\)/);
const long=Array.from({length:40},(_,i)=>'Answer line '+(i+1)).join('\n');
const longOp={project:'p',op:'answer',task:1058,text:'PM: '+'a long owner answer '.repeat(30)};
const cases={
  result:[{state:'result',text:long+'\nNext: 1 Open #41 ★ · 2 Start #44'},['data-a="ai-next" data-v="1"','data-a="ai-send"','<textarea id="ai"'],[]],
  clarify:[{state:'clarify',text:'Which project?'},['>Reply</button>','placeholder="Answer…"'],[]],
  proposal:[{state:'proposal',rev:3,text:'Plan ready.',ops:[{project:'p',op:'add',title:'T'},longOp]},['data-a="ai-confirm"','>Confirm: apply 2 changes</button>'],['p: add "T"','p: answer #1058 PM: a long owner answer']],
  failed:[{state:'failed',reason:'timeout',text:long},['>Edit and send again</button>','data-a="ai-send"'],[]],
};
const block=(h,cls)=>{
  const start=h.indexOf('<div class="'+cls);
  if(start<0)return null;
  let depth=0;
  for(const m of h.slice(start).matchAll(/<div\b|<\/div>/g)){depth+=m[0]==='</div>'?-1:1;if(!depth)return [start,start+m.index];}
};
for(const [name,[answer,controls,ops]] of Object.entries(cases)){
  const request={id:'2',root:'p',text:'Ask',answer,chain:[{text:'Older',answer:{state:'clarify',text:'Older question'}},{text:'Ask',answer}]};
  const context={S:{projects:[],ai:{target:'p',history:[],request}},esc:s=>String(s).replaceAll('<','&lt;'),aiText:'',aiMenu:false,aiConfirm:null};
  vm.createContext(context);vm.runInContext(source,context);
  const h=vm.runInContext('ai()',context);
  const chain=block(h,'aichain');
  assert(chain,name);
  const inside=h.slice(...chain);
  assert(inside.includes('Older question')&&inside.includes(answer.text.split('\n')[0]),name+': the exchange is in the preview');
  for(const c of controls)assert(h.indexOf(c)>chain[1],name+': '+c+' stays below the preview');
  const body=block(h,'body meta aiops');
  assert.equal(!!body,ops.length>0,name+': proposed changes are bounded');
  if(!body)continue;
  assert(body[0]>h.indexOf('data-a="ai-send"'),name+': proposed changes sit under Send');
  for(const o of ops)assert(h.slice(...body).includes(o),name+': '+o+' is in the bounded changes');
  for(const c of controls)assert(h.indexOf(c)>body[1],name+': '+c+' stays below the changes');
}
console.log('PASS bounded Home AI previews; Next, Reply, Confirm and Edit stay outside them');
