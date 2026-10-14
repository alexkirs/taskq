// Chain: src/answer-card.{js,css} in a real browser. Risk: clipped controls at 16/20 px or narrow widths, a refresh
// that drops the draft or focus, a double click that sends twice. Instrument, not a CI gate (needs Playwright):
// npm i --no-save playwright && node tests/answer-card.browser.cjs
const fs = require('node:fs'), assert = require('node:assert/strict');
const {chromium} = require('playwright');
const js = fs.readFileSync('src/answer-card.js', 'utf8'), css = fs.readFileSync('src/answer-card.css', 'utf8');
const fx = JSON.parse(fs.readFileSync('tests/fixtures/answer-card.json', 'utf8'));
(async () => {
  const browser = await chromium.launch(require('./browser.cjs')());
  for (const scheme of ['light', 'dark']) for (const font of [16, 20]) for (const width of [280, 480]) {
    const page = await browser.newPage({viewport: {width, height: 900}, colorScheme: scheme});
    await page.setContent('<!doctype html><meta name="color-scheme" content="light dark"><style>' + css + 'body{margin:8px;font:' + font + 'px/1.4 sans-serif;background:light-dark(#fafbfc,#252b32)}</style><div id="c"></div><script>' + js + '</script>');
    const at = scheme + ' ' + font + 'px ' + width + 'px';
    // every fixture checkpoint fits: nothing wider than the column, every control at least one line tall
    const html = fx.cases.flatMap(c => TaskqReplay(c)).join('');
    function TaskqReplay(c) { return require('../src/answer-card.js').replay(c, (_, r) => r.capability || c.capability).map(x => x.html); }
    const fit = await page.evaluate(h => {
      const c = document.getElementById('c'); c.innerHTML = h;
      const bad = [...c.querySelectorAll('.qac *')].filter(e => e.getBoundingClientRect().right > c.getBoundingClientRect().right + 0.5).map(e => e.outerHTML.slice(0, 80));
      const small = [...c.querySelectorAll('.qac button')].filter(b => b.getBoundingClientRect().height < parseFloat(getComputedStyle(b).fontSize)).length;
      const fg = getComputedStyle(c.querySelector('.qac')).color;
      return {bad, small, overflow: document.documentElement.scrollWidth > innerWidth, fg};
    }, html);
    assert.deepEqual(fit.bad, [], at);
    assert.equal(fit.small, 0, at);
    assert.equal(fit.overflow, false, at);
    assert.equal(fit.fg, scheme === 'light' ? 'rgb(36, 48, 57)' : 'rgb(238, 242, 245)', at);
    // live binding: draft and focus survive refresh, a double click sends once, a new revision holds the draft
    const r = await page.evaluate(async rows => {
      const c = document.getElementById('c'), sent = [];
      let release;
      const ui = TaskqAnswerCard.mount(c, {capability: () => ({answer: true}), send: (_, op) => { sent.push(op); return new Promise(r => { release = r; }); }});
      ui.update([{board: 'b', row: rows[0]}]);
      c.querySelector('.qac-other').open = true; const t = c.querySelector('textarea'); t.focus(); t.value = 'keep it'; t.dispatchEvent(new Event('input', {bubbles: true}));
      t.setSelectionRange(2, 4);
      ui.update([{board: 'b', row: rows[0]}]);
      const kept = {draft: c.querySelector('textarea').value, focus: document.activeElement === c.querySelector('textarea'), sel: [document.activeElement.selectionStart, document.activeElement.selectionEnd]};
      ui.update([{board: 'b', row: rows[1]}]);
      const stale = {state: c.querySelector('.qac').dataset.qacState, draft: c.querySelector('textarea').value, send: c.querySelector('[type=submit]').disabled};
      c.querySelector('[data-qac=review]').click();
      c.querySelector('[data-choice="2"]').click(); c.querySelector('[data-choice="2"]').click();
      await new Promise(r => setTimeout(r, 0));
      const sending = c.querySelector('.qac').dataset.qacState;
      release({state: 'accepted'}); await new Promise(r => setTimeout(r, 0));
      return {kept, stale, sending, sent: sent.length, done: c.querySelector('.qac').dataset.qacState};
    }, fx.cases.find(c => c.name.startsWith('stale')).rows);
    assert.deepEqual(r, {kept: {draft: 'keep it', focus: true, sel: [2, 4]}, stale: {state: 'stale', draft: 'keep it', send: true}, sending: 'sending', sent: 1, done: 'accepted'}, at);
    // keyboard: Tab walks choices in source order, then the Other answer and context disclosures (Q1)
    await page.evaluate(row => { TaskqAnswerCard.mount(document.getElementById('c'), {capability: () => ({answer: true}), send: () => new Promise(() => {})}).update([{board: 'k', row}]); document.querySelector('.qac-head a').focus(); }, fx.cases[0].rows[0]);
    const order = [];
    for (let i = 0; i < 4; i++) { await page.keyboard.press('Tab'); order.push(await page.evaluate(() => document.activeElement.textContent)); }
    assert.deepEqual(order, ['1. Keep compatibility ★', '2. Remove the old API', 'Other answer', 'Checked & context'], at + ': source order');
    await page.screenshot({path: require('node:os').tmpdir() + '/answer-card-' + scheme + '-' + font + '-' + width + '.png', fullPage: true});
    await page.close();
    console.log('PASS', at);
  }
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
