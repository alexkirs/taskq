// Chain: taskq.resources/1 projection -> shared fixtures -> src/resource-controls.js states. Risk: unknown reads as
// zero or Ready, a legacy guard reads as Auto, an untouched field travels, a retry mints a new operation.
// Run: node tests/resource-controls.test.cjs. Consumers (tray, DashQ) replay the same fixture file.
const fs = require('node:fs'), crypto = require('node:crypto'), assert = require('node:assert/strict');
const rc = require('../src/resource-controls.js');
const fx = JSON.parse(fs.readFileSync('tests/fixtures/resource-controls.json', 'utf8'));
for (const c of fx.cases) {
  const checks = c.events.filter(e => e.type === 'check');
  const got = rc.replay(c, () => null);
  assert.equal(got.length, checks.length, c.name);
  checks.forEach((want, i) => {
    const g = got[i], at = c.name + ' check ' + (i + 1);
    if (want.status) assert.equal(g.status, want.status, at);
    if (want.headline) assert.equal(g.headline, want.headline, at);
    if (want.sent != null) assert.equal(g.sent.length, want.sent, at);
    for (const s of want.has || []) assert(g.html.includes(s), at + ' lacks ' + s + '\n' + g.html);
    for (const s of want.not || []) assert(!g.html.includes(s), at + ' has ' + s + '\n' + g.html);
    const last = g.sent[g.sent.length - 1];
    if (want.ops) assert.deepEqual(last.ops, want.ops, at + ': one contract-shaped change per target, untouched fields stay');
    if (want.same_op) assert.deepEqual(last, g.sent[g.sent.length - 2], at + ': a retry keeps the id and payload');
  });
  assert(rc.STATES.includes(got[got.length - 1].status));
  console.log('PASS', c.name);
}
// A3: the published assets match the checksums the design doc records
const doc = fs.readFileSync('docs/design/shared-resource-controls.md', 'utf8');
for (const f of ['src/resource-controls.js', 'src/resource-controls.css', 'tests/fixtures/resource-controls.json']) {
  const h = crypto.createHash('sha256').update(fs.readFileSync(f, 'utf8').replace(/\r\n/g, '\n')).digest('hex');
  assert(doc.includes('`' + f + '` SHA-256: `' + h + '`'), f + ' checksum ' + h + ' is not recorded in docs/design/shared-resource-controls.md');
}
console.log('PASS asset checksums');
