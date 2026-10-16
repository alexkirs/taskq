// Chain: #1115 Home proposal preview under Send in src/tray.html. Risk: a long proposed op fills Home and hides Confirm and Projects, or a bounded block cuts a line in half.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const html=fs.readFileSync('src/tray.html','utf8');
const source=html.slice(html.indexOf('const aiStates='),html.indexOf('// Add project'));
// Actual scroll and whole-line bounds are checked by tray-home-proposal.browser.cjs.
const longOp={project:'p',op:'answer',task:1058,text:'PM: '+'a long owner answer '.repeat(30)};
const answer={state:'proposal',rev:2,text:'Plan ready.',ops:[{project:'p',op:'add',title:'T'},longOp]};
const request={id:'2',root:'p',text:'Ask',answer,chain:[{text:'Older',answer:{state:'clarify',text:'Which?'}},{text:'Ask',answer}]};
const context={S:{projects:[],ai:{target:'p',history:[],request}},esc:s=>String(s).replaceAll('<','&lt;'),aiText:'',aiMenu:false,aiConfirm:null};
vm.createContext(context);vm.runInContext(source,context);
const h=vm.runInContext('ai()',context);
const start=h.indexOf('<div class="body meta aiops"');
assert(start>h.indexOf('data-a="ai-send"'),'proposed changes sit under Send');
const end=start+h.slice(start).indexOf('</div>');
for(const o of ['p: add "T"','p: answer #1058 PM: a long owner answer'])assert(h.slice(start,end).includes(o),o+' is in the bounded changes');
// two exchanges with rev equal to the exchange count: each block keeps its own scroll key
const keys=[...h.matchAll(/data-bound="([^"]*)"/g)].map(m=>m[1]);
assert.equal(new Set(keys).size,2,'bound keys differ even when chain.length == answer.rev: '+keys);
assert(h.indexOf('>Confirm: apply 2 changes</button>')>end,'Confirm stays below the changes');
console.log('PASS bounded Home proposal preview; Confirm stays outside it');
