// Run: node docs/design/menu-prototype.test.cjs
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const path = require('node:path');
const html = fs.readFileSync(path.join(__dirname, 'menu-prototype.html'), 'utf8');
const context = {};
vm.runInNewContext(html.match(/<script>([\s\S]*?)<\/script>/)[1], context);
const m = context.taskqMenuModel;
const normalize = value => JSON.parse(JSON.stringify(value));
let checks = 0;
function test(name, fn) { fn(); checks++; console.log('PASS', name); }
test('M-A2: overlapping scopes do not duplicate counts', () => {
  assert.deepEqual(normalize(m.counts([...m.fixtures, ...m.fixtures])), { work: 1, ask: 1, ready: 1, failed: 1 });
  const sameNumberOtherRepo = { ...m.fixtures[0], key: 'gitlab.example/other/repo/664' };
  assert.equal(m.counts([...m.fixtures, sameNumberOtherRepo]).work, 2);
});
test('M-A2: review is work; waiting is not a question', () => {
  const rows = ['review', 'waiting', 'later'].map((state, i) => ({ key: String(i), state }));
  assert.deepEqual(normalize(m.counts(rows)), { work: 1, ask: 0, ready: 0, failed: 0 });
});
test('M-A6: event identity, half-open range and failed transitions', () => {
  const input = [{attemptId:'a', time:0, kind:'close'}, {attemptId:'a', time:0, kind:'close'}, {attemptId:'b', time:8, kind:'failed'}, {attemptId:'c', time:9, kind:'close'}, {attemptId:'d', time:-1, kind:'close'}];
  const bins = m.histogram(input, 0, 9);
  assert.equal(bins[0].close, 1);
  assert.equal(bins[8].failed, 1);
  assert.equal(bins.reduce((n,b) => n+b.close+b.failed, 0), 2);
});
test('M-A5: changing history interval leaves current failed count intact', () => {
  assert.equal(m.histogram([], 0, 9).reduce((n,b) => n+b.failed, 0), 0);
  assert.equal(m.counts(m.fixtures).failed, 1);
});
test('M-A6: invalid custom intervals are refused', () => {
  for(const pair of [[5,5], [9,0], [NaN,9], [0,Infinity]]) assert.throws(() => m.histogram([], ...pair));
});
test('M-A7 model: stale confirmation cannot apply; duplicate cannot apply twice', () => {
  const state = { proposal:{id:'r1',revision:2}, applied:[] };
  assert.equal(m.confirm(state,'r1',1), 'stale');
  assert.equal(state.applied.length,0);
  assert.equal(m.confirm(state,'r1',2), 'accepted');
  assert.equal(m.confirm(state,'r1',2), 'duplicate');
  assert.deepEqual(state.applied,['r1']);
});
test('D-A1/D-A2/M-A3: one linked row contains number and title, not a metadata subrow', () => {
  for(const t of m.fixtures) {
    const row = m.taskRow(t);
    assert.equal((row.match(/<a /g)||[]).length,1);
    assert(row.includes('href="'+t.url+'"'));
    assert(row.includes('#'+t.number));
    assert(!/<br|<small|<button/.test(row));
  }
});
test('D-A2: global rows identify their project; external text is escaped', () => {
  const row = m.taskRow({...m.fixtures[0], title:'<img onerror="bad"> & text'}, true);
  assert(row.includes('taskq'));
  assert(!row.includes('<img'));
  assert(row.includes('&lt;img'));
});
test('D-A3/M-A1 static contract: adjustable font and both modes remain present', () => {
  assert(html.includes('[14,16,18,20]'));
  assert(html.includes("view:'icon'"));
  assert(html.includes('data-view="icon"'));
  assert(html.includes('data-view="full"'));
});
test('M-A12: project service menu retains legacy actions and scoped confirmation', () => {
  const menu = m.serviceMenu('taskq');
  for (const label of ['Open log','Open folder','Restart','Stop until next logon','Confirm uninstall taskq']) assert(menu.includes(label));
  assert(menu.includes('disabled'));
  assert(menu.includes('native service actions are not connected'));
  assert(menu.includes('Update now'));
  assert(menu.includes('.taskq/update'));
  assert(!m.serviceMenu('serverq').includes('Update now'));
  assert(!m.serviceMenu('taskq',null).includes('Update now'));
  assert(m.serviceMenu('taskq','4.0.12').includes('4.0.12'));

});
test('M-A13: history keeps newest, deduplicates, ignores blank and bounds to 20', () => {
  assert.deepEqual(normalize(m.remember(['one','two'], ' two ')), ['two','one']);
  assert.deepEqual(normalize(m.remember(['one'], '  ')), ['one']);
  assert.equal(m.remember(Array.from({length:20},(_,i)=>String(i)), 'new').length,20);
});
test('D-A8: counters distinguish zero and preserve multi-digit values', () => {
  assert(m.counter(0,'ask','Ответ').includes('zero'));
  for(const n of [1,12,100]) { const pill=m.counter(n,'ask','Ответ'); assert(pill.includes('ask filled')); assert(pill.includes('>'+n+'</span>')); }
});
test('M-A13: history button and recall stay separate from sending', () => {
  assert(!html.includes('Scope example'));
  assert(html.includes('aria-label="History and commands"'));
  assert(html.includes('data-recall='));
  assert(html.includes('data-preset='));
  assert(html.includes('history:state.history'));
});
test('M-A1: Windows uses compact q; macOS may select summary', () => {
  for (const view of ['icon','full']) assert.equal(m.summaryVisible('windows',view),false);
  assert.equal(m.summaryVisible('macos','full'),true);
  assert.equal(m.summaryVisible('macos','icon'),false);
  assert(html.includes("platform==='windows'?'hidden':''"));
  assert(html.includes('height:22pt'));
  assert(html.includes('grid-template-rows:12pt 7pt'));
});
test('M-A12: missing saved focus asks for labels without enabling all', () => {
  for (const focus of [null,undefined,'',' ','off']) assert.equal(m.resumeFocus(focus),null);
  assert.equal(m.resumeFocus('area-ui'),'area-ui');
  assert.equal(m.resumeFocus('all'),'all');
});
test('M-A6: incomplete local history is unknown with an explicit start date', () => {
  assert.equal(m.historyComplete(0,1),false);
  assert.equal(m.historyComplete(1,1),true);
  assert.equal(m.historyComplete(1,null),false);
  assert(html.includes('History since '));
  assert(html.includes('Earlier history is unknown, not zero.'));
  for (const event of m.events) { assert(event.attemptId); assert(event.task); assert(['close','failed'].includes(event.kind)); }
});
test('M-A14: Add project is phase 4 and keeps all folder classifications', () => {
  const markup=m.addProjectMenu();
  for(const text of ['phase 4','Empty folder','Files without taskq.json','passing selftest','Failed selftest or old format','Exact old-project marker remains open']) assert(markup.includes(text));
  assert(markup.includes('disabled'));
});
test('Product HTML has English strings and no ChatGPT widget dependency', () => {
  assert(!/[\u0400-\u04ff]/u.test(html));
  assert(!/window\.openai|widgetState|openai:set_globals/.test(html));
  assert(html.includes('taskqMenuBridge'));
  assert(html.includes('install/projects'));
  assert(html.includes('wry webview'));
});
test('M-A1/M-A4/M-A7/M-A9/M-A13: fixture state/action seam preserves user intent and confirmation', () => {
  const elements={'.menu':{style:{setProperty(){}},innerHTML:''},'.trigger':{innerHTML:'',addEventListener(){},setAttribute(){}}};
  const root={querySelector:key=>elements[key],querySelectorAll:()=>[],appendChild(){},addEventListener(){}};
  const store=new Map();
  const browser={document:{getElementById:()=>root,createElement:()=>({style:{},remove(){}})},location:{search:'?platform=windows'},navigator:{userAgent:'Windows'},URLSearchParams,getComputedStyle:()=>({color:'#777'}),localStorage:{getItem:key=>store.get(key),setItem:(key,value)=>store.set(key,value)}};
  vm.runInNewContext(html.match(/<script>([\s\S]*?)<\/script>/)[1],browser);
  const bridge=browser.taskqMenuBridge;
  bridge.receiveState({view:'full',paused:true,savedFocus:null,request:null});
  assert(!elements['.trigger'].innerHTML.includes('micro'));
  assert(elements['.menu'].innerHTML.includes('Original request not recorded.'));
  bridge.action({action:'pause'});
  assert.equal(bridge.state().paused,true);
  assert(elements['.menu'].innerHTML.includes('Choose labels before resuming.'));
  bridge.action({action:'choose-focus'});
  assert.equal(bridge.state().savedFocus,'area-ui');
  assert.equal(bridge.state().paused,false);
  bridge.action({preset:'1'});
  assert.equal(bridge.state().history.length,0);
  bridge.action({action:'send'});
  assert.equal(bridge.state().history.length,1);
  assert(bridge.state().proposal);
  bridge.action({action:'confirm'});
  assert.equal(bridge.state().paused,true);
  assert(bridge.state().reply.includes('No service write'));
  assert(store.has('taskq-menu-fixture'));
});
console.log(`${checks} fixture checks passed. DOM interaction, visual, native OS and service behavior are NOT qualified.`);
