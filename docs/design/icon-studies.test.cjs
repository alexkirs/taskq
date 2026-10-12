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

assert(!/[\u0400-\u04ff]/u.test(html), 'All study strings must be English.');
assert(!html.includes('window.openai'), 'The local study must not depend on a host bridge.');
assert(html.includes('Windows supports only the compact q; summary and the M1 appearance chooser are hidden.'));
assert(html.includes('padding:0 12px;height:22px'));
assert(html.includes('height:19px;display:grid'));
assert(html.includes('grid-template-rows:12px 7px'));
assert(html.includes('grid-column:1/3;height:12px'));
assert(html.includes('.graph{display:flex;align-items:flex-end;height:7px;gap:1px}'));
assert(html.includes('.q{width:18px;height:18px;border:1.8px solid currentColor;border-radius:1.8px'));
assert(html.includes('width:1.8px;height:4.5px;right:-1.8px;bottom:-3.6px'));
assert(html.includes('transform:translate(-50%,-50%);width:18px;height:18px'));
assert((22-18)/2 + 18-1.8+3.6 <= 22, 'Compact q including its tail fits the menu bar.');
assert.equal(heights(3,3,6).reduce((sum,height)=>sum+height,0), 7);

// Execute the real render and pause handler with canvas metrics of unequal glyph widths.
let toggle;
const content = { innerHTML: '' };
const draws = [];
const metrics = [{actualBoundingBoxLeft:1,actualBoundingBoxRight:6},{actualBoundingBoxLeft:0,actualBoundingBoxRight:6},{actualBoundingBoxLeft:2,actualBoundingBoxRight:6}];
const ctx = {
  scale(x,y) { assert.equal(x,3); assert.equal(y,3); },
  measureText(text) { return metrics[['3','2','5'].indexOf(text)]; },
  fillText(text,x,y) { draws.push({text,x,y,font:this.font}); },
};
const root = {
  appendChild() {},
  querySelectorAll(selector) {
    if(selector === '.study') return [{dataset:{kind:'ribbon'},querySelector(selector) {
      return selector === '.content' ? content : {addEventListener(event,callback) { assert.equal(event,'click'); toggle=callback; }};
    }}];
    return [...content.innerHTML.matchAll(/<canvas data-width="(\d+)"[^>]* width="(\d+)" height="(\d+)"/g)].map(([,width,pixels,height])=>{
      assert.equal(Number(pixels),Number(width)*3);
      assert.equal(Number(height),12*3);
      return {dataset:{width},getContext:()=>ctx,closest:()=>root};
    });
  },
};
vm.runInNewContext(html.match(/<script>([\s\S]*?)<\/script>/)[1], {
  document:{getElementById:()=>root,createElement:()=>({style:{},remove(){}})},
  getComputedStyle:()=>({color:'#123456'}),
});
const running = content.innerHTML;
assert(running.includes('bar dark') && running.includes('bar light'));
assert(running.includes('aria-pressed="false">Pause'));
assert(!running.includes('pause-overlay'));
assert.equal(draws.length,9);
for(let i=0;i<draws.length;i+=3) {
  assert.equal(draws[i].x-metrics[0].actualBoundingBoxLeft,0);
  assert.equal(draws[i+2].x+metrics[2].actualBoundingBoxRight,47);
  assert(draws.slice(i,i+3).every(d=>d.y===11 && d.font.startsWith('600 12px')));
}
const graphs = markup => [...markup.matchAll(/<span class="graph">[\s\S]*?<\/span><\/span>/g)].map(match=>match[0]);
toggle();
assert(content.innerHTML.includes('aria-pressed="true">Resume'));
assert.equal((content.innerHTML.match(/class="pause-overlay"/g)||[]).length,3);
assert.equal((content.innerHTML.match(/class="wide ribbon is-paused"/g)||[]).length,3);
assert.deepEqual(graphs(content.innerHTML),graphs(running));
toggle();
assert.equal(content.innerHTML,running);
console.log('PASS: English local study; platform note; 22 pt bar, 12 pt digits, 7 pt graph; shared visible edges; pause preserves history and resumes.');
