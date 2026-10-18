// B4 parent/child comparison (docs/design/shared-blocks.md): the parent question card in docs/design/workspace-preview.html
// against the tray's shared card on the same task. Risk: the child drifts from parent tokens, choice layout or
// disclosures, or the card breaks the project counter strip (K-A8). Instrument, not a CI gate (needs Playwright):
// npm i --no-save playwright && node tests/answer-card.parent.cjs
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict'), {chromium} = require('playwright');
const html=fs.readFileSync('build/web/tray.html','utf8');
const fx = JSON.parse(fs.readFileSync('tests/fixtures/answer-card.json', 'utf8'));
// the computed look both cards must share; selectors map parent qcard -> child qac
function look(sel) {
  const q = document.querySelector(sel.card), g = (e, k) => getComputedStyle(e)[k], b = q.querySelector(sel.choice), list = q.querySelector(sel.choices);
  return {fg: g(q, 'color'), divider: g(q, 'borderTopColor'), mark: g(q.querySelector(sel.mark), 'color'), choiceFill: g(b, 'backgroundColor'),
    choiceBorder: g(b, 'borderTopWidth'), radius: g(b, 'borderTopLeftRadius'), padding: g(b, 'padding'), minHeight: g(b, 'minHeight'),
    number: g(q.querySelector(sel.number), 'fontVariantNumeric'), numberText: q.querySelector(sel.number).textContent, gap: g(list, 'gap'),
    wrap: g(list, 'flexWrap'), star: b.textContent.trim().endsWith('★'), disclosures: [...q.querySelectorAll('summary')].map(s => s.textContent)};
}
(async () => {
  const browser = await chromium.launch(require('./browser.cjs')());
  try {
    for (const theme of ['light', 'dark']) {
      const parent = await browser.newPage({viewport: {width: 1024, height: 1200}, colorScheme: theme});
      await parent.goto('file://' + path.resolve('docs/design/workspace-preview.html'));
      const frame = parent.frames()[1];
      await frame.waitForSelector('.menu [data-question=task]');
      const want = await frame.evaluate(look, {card: '.menu [data-question=task]', choice: '.qchoices button', choices: '.qchoices', mark: '.ask', number: '.qnumber'});
      await parent.close();
      for (const font of [16, 20]) for (const width of [320, 360]) for (const n of [0, 1, 12, 100]) {
        const at = theme + ' ' + font + 'px ' + width + 'px count ' + n;
        const page = await browser.newPage({viewport: {width, height: 900}, colorScheme: theme});
        const errors = []; page.on('pageerror', e => errors.push(e.message));
        await page.evaluate(() => { window.ipc = {postMessage() {}}; });
        await page.setContent(html);
        const row = fx.cases.find(c => c.name.startsWith('choice')).rows[0], filler = (state, k) => Array.from({length: k}, (_, i) => ({...row, n: 10000 * (state === 'ready' ? 2 : 1) + i, state, title: 'Long task title that wraps at large type ' + i, card: null, question: null}));
        const p = {root: 'p', name: 'project-with-a-long-name', issues: 'https://github.com/o/r/issues', alive: true, complete: true, focus: [], labels: [], tasks: [row, ...filler('doing', n), ...filler('ready', n)], cap_saved: {error: 'not configured'}};
        await page.evaluate(s => taskq.receive(s), {page: 'p', font, projects: [p], ai: {draft: ''}, notify: {projects: {}}});
        const got = await page.evaluate(look, {card: '.qac', choice: '.qac-choices button', choices: '.qac-choices', mark: '.qac-mark', number: '.qac-number'});
        assert.deepEqual(got, want, at + ': child look equals parent');
        // K-A8: the project strip ends at its row's right edge, state order kept, nothing wider than the window
        const strip = await page.evaluate(() => {
          const s = document.querySelector('.tqp-project .tqp-counts'), r = s.closest('.tqp-project');
          return {gap: Math.round(r.getBoundingClientRect().right - parseFloat(getComputedStyle(r).paddingRight) - s.getBoundingClientRect().right),
            order: [...s.querySelectorAll('button')].map(c => c.dataset.state), overflow: document.documentElement.scrollWidth > innerWidth};
        });
        assert.deepEqual(strip, {gap: 0, order: [...(n?['doing']:[]), 'ask', ...(n?['ready']:[]), 'closed'], overflow: false}, at);
        assert.deepEqual(errors, [], at);
        await page.close();
      }
      console.log('PASS parent/child look, K-A8 strip, no overflow;', theme, '16/20 px, 320/360 px, counts 0/1/12/100');
    }
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exit(1); });
