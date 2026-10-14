// Chain: § 4 tray clarify reply in src/tray.html. Risk: wrong thread order, unescaped draft or Send enabled without text; a long answer pushes Projects off Home.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const html=fs.readFileSync('src/tray.html','utf8');
const source=html.slice(html.indexOf('const aiStates='),html.indexOf('// Add project'));
const context={S:{projects:[],ai:{target:'p',history:[],request:{id:'2',root:'p',text:'reply',answer:{state:'clarify',text:'Newest question'},chain:[{text:'first',answer:{state:'clarify',text:'Older question'}},{text:'reply',answer:{state:'clarify',text:'Newest question'}}]}}},esc:s=>String(s).replaceAll('<','&lt;'),aiText:'<draft>',aiMenu:false,aiConfirm:null};
vm.createContext(context);vm.runInContext(source,context);
let h=vm.runInContext('ai()',context);
assert(h.indexOf('Older question')<h.indexOf('Newest question'));
assert(h.indexOf('Newest question')<h.indexOf('<textarea'));
// #1059: the exchange is one bounded, scrollable preview; reply and confirm controls stay outside it
assert(h.indexOf('<div class="aichain"')<h.indexOf('Older question')&&h.indexOf('Newest question')<h.indexOf('</div></div></div><div class="askai">'));
assert(h.includes('placeholder="Answer\u2026"'));
assert(h.includes('aria-label="Answer to AI"'));
assert(h.includes('&lt;draft>'));
assert(h.includes('<div class="options"><button data-a="ai-send">Reply</button>'));
context.S.ai.request=null;context.S.ai.target=null;
h=vm.runInContext('ai()',context);
assert(h.includes('placeholder="Ask, or request a change"'));
assert(h.includes('data-a="ai-send" disabled>Send'));
console.log('PASS clarify ordering, bounded exchange, answer composer, visible Reply, draft escaping, disabled Send');
