const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const path = require('node:path');
const html = fs.readFileSync(path.join(__dirname, 'icon-studies.html'), 'utf8');
const context = {};
vm.runInNewContext(html.match(/<script>([\s\S]*?)<\/script>/)[1], context);
const { heights } = context.taskqIconStudy;
assert.equal(heights(3,3,6)[0], heights(3,3,6)[1]);
assert.equal(heights(3,0,6)[0], heights(0,3,6)[1]);
assert.equal(heights(0,0,6)[0], 0);
assert.equal(heights(1,0,6)[0] * 3, heights(3,0,6)[0]);
console.log('PASS: equal status quantities, common scale across intervals, zero, linear proportions. Visual rasterization not verified.');

const totals = n => context.taskqIconStudy.aggregate(n).reduce((a,b)=>[a[0]+b[0],a[1]+b[1]],[0,0]);
assert.deepEqual(totals(8),totals(9));
assert.deepEqual(totals(12),totals(9));
console.log("PASS: compact variants preserve event totals across bin counts.");

for (const n of [8,9,12]) { const bins=context.taskqIconStudy.aggregate(n); assert(bins[0][0]+bins[0][1]>0); assert(bins[n-1][0]+bins[n-1][1]>0); }
assert(html.includes("left:50%;top:50%;transform:translate(-50%,-50%)"));
assert(!html.includes("opacity:.72"));
console.log("PASS: edge fixtures populated; overlay centered on both axes; pause has no opacity reduction.");

assert(html.includes('class="pause-overlay" aria-hidden="true">'+"'+small()+'"));
assert(!html.includes(".pause-overlay:before"));
console.log("PASS: overlay reuses the compact q instead of a separate pause drawing.");
