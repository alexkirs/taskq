// Chain: canonical question (cargo test) -> shared fixtures -> src/answer-card.js states. Risk: a duplicate click sends
// twice, a stale draft sends to a new question, or a consequential choice is answerable outside the board.
// Run: node tests/answer-card.test.cjs. Consumers (tray, DashQ) run the same replay over the same fixture file.
const fs = require('node:fs'), crypto = require('node:crypto'), assert = require('node:assert/strict');
const card = require('../src/answer-card.js');
const fx = JSON.parse(fs.readFileSync('tests/fixtures/answer-card.json', 'utf8'));
for (const c of fx.cases) {
  const checks = c.events.filter(e => e.type === 'check');
  const got = card.replay(c, (_, row) => row.capability || c.capability);
  assert.equal(got.length, checks.length, c.name);
  checks.forEach((want, i) => {
    const g = got[i], at = c.name + ' check ' + (i + 1);
    if (want.status) assert.equal(g.status, want.status, at);
    if (want.sent != null) assert.equal(g.sent.length, want.sent, at);
    if (want.draft != null) assert.equal(g.draft, want.draft, at);
    for (const s of want.has || []) assert(g.html.includes(s), at + ' lacks ' + s + '\n' + g.html);
    for (const s of want.not || []) assert(!g.html.includes(s), at + ' has ' + s + '\n' + g.html);
    const last = g.sent[g.sent.length - 1];
    if (want.op) for (const [k, v] of Object.entries(want.op)) assert.equal(last[k], v, at + ' op.' + k);
    if (want.same_op) assert.deepEqual(last, g.sent[g.sent.length - 2], at + ': a retry keeps the id and payload');
    if (want.new_op) assert.notEqual(last.id, g.sent[g.sent.length - 2].id, at + ': a changed answer is a new operation');
  });
  assert(card.STATES.includes(got[got.length - 1].status));
  console.log('PASS', c.name);
}
// A2: the published assets match the checksums the design doc records
const doc = fs.readFileSync('docs/design/answer-card.md', 'utf8');
for (const f of ['src/answer-card.js', 'src/answer-card.css', 'tests/fixtures/answer-card.json']) {
  const h = crypto.createHash('sha256').update(fs.readFileSync(f, 'utf8').replace(/\r\n/g, '\n')).digest('hex');
  assert(doc.includes('`' + f + '` SHA-256: `' + h + '`'), f + ' checksum ' + h + ' is not recorded in docs/design/answer-card.md');
}
console.log('PASS asset checksums');
