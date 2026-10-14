// Chain: #1059 Home inline AI exchange in src/tray.html. Risk: a long answer fills Home and pushes Projects off it, or the bounded preview swallows reply and confirm controls.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const html=fs.readFileSync('src/tray.html','utf8');
const source=html.slice(html.indexOf('const aiStates='),html.indexOf('// Add project'));
// the preview is bounded and scrolls inside itself
assert.match(html,/\.aichain\{max-height:10em;overflow-y:auto[;}]/);
const long=Array.from({length:40},(_,i)=>'Answer line '+(i+1)).join('\n');
const cases={
  result:[{state:'result',text:long+'\nNext: 1 Open #41 ★ · 2 Start #44'},['data-a="ai-next" data-v="1"','data-a="ai-send"','<textarea id="ai"']],
  clarify:[{state:'clarify',text:'Which project?'},['>Reply</button>','placeholder="Answer…"']],
  proposal:[{state:'proposal',rev:3,text:'Plan ready.',ops:[{project:'p',op:'add',title:'T'}]},['data-a="ai-confirm"','>Confirm: apply 1 change</button>']],
  failed:[{state:'failed',reason:'timeout',text:long},['>Edit and send again</button>','data-a="ai-send"']],
};
for(const [name,[answer,controls]] of Object.entries(cases)){
  const request={id:'2',root:'p',text:'Ask',answer,chain:[{text:'Older',answer:{state:'clarify',text:'Older question'}},{text:'Ask',answer}]};
  const context={S:{projects:[],ai:{target:'p',history:[],request}},esc:s=>String(s).replaceAll('<','&lt;'),aiText:'',aiMenu:false,aiConfirm:null};
  vm.createContext(context);vm.runInContext(source,context);
  const h=vm.runInContext('ai()',context);
  const start=h.indexOf('<div class="aichain"');
  assert(start>0,name);
  let depth=0,end=start;
  for(const m of h.slice(start).matchAll(/<div\b|<\/div>/g)){depth+=m[0]==='</div>'?-1:1;if(!depth){end=start+m.index;break;}}
  const inside=h.slice(start,end);
  assert(inside.includes('Older question')&&inside.includes(answer.text.split('\n')[0]),name+': the exchange is in the preview');
  for(const c of controls)assert(h.indexOf(c)>end,name+': '+c+' stays below the preview');
}
console.log('PASS bounded Home AI preview; Next, Reply, Confirm and Edit stay outside it');
