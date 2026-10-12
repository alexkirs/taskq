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
  const input = [{id:'a', at:0, kind:'done'}, {id:'a', at:0, kind:'done'}, {id:'b', at:8, kind:'failed'}, {id:'c', at:9, kind:'done'}, {id:'d', at:-1, kind:'done'}];
  const bins = m.histogram(input, 0, 9);
  assert.equal(bins[0].done, 1);
  assert.equal(bins[8].failed, 1);
  assert.equal(bins.reduce((n,b) => n+b.done+b.failed, 0), 2);
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
  for (const label of ['Открыть лог','Открыть папку','Перезапустить','Остановить до следующего входа','Подтвердить удаление taskq']) assert(menu.includes(label));
  assert(menu.includes('disabled'));
  assert(menu.includes('системные действия не подключены'));
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
test('M-A13: history button replaces scope example; recall is separate from send', () => {
  assert(!html.includes('Пример scope'));
  assert(html.includes('aria-label="История и команды"'));
  assert(html.includes('data-recall='));
  assert(html.includes('data-preset='));
  assert(html.includes('history:state.history'));
});
console.log(`${checks} fixture checks passed. DOM interaction, visual, native OS and service behavior are NOT qualified.`);
