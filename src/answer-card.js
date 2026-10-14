// TaskQ shared answer card (docs/design/answer-card.md). Source: https://github.com/taskqorg/taskq src/answer-card.js,
// MIT, published with each TaskQ release as taskq-<version>-answer-card.js and listed in SHA256SUMS; never edit a copy.
// Input: one status --json task row whose `question` is TaskQ's canonical parse, plus the adapter's capability.
// The component has no network client, parser, credentials or task state machine: `send` is the adapter's.
(function (root) {
  'use strict';
  const STATES = ['actionable', 'sending', 'accepted', 'stale', 'rejected', 'unknown', 'read-only'];
  const LABELS = {actionable: 'Waiting for your answer', sending: 'Sending…', accepted: 'Answer accepted on the board',
    stale: 'The question changed: review it before sending', rejected: 'Answer not accepted', unknown: 'Delivery unknown: check the board', 'read-only': 'Read only'};
  const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]));
  const key = (board, row) => board + '#' + row.n;
  // only web links: card text is worker-written
  const href = u => /^https?:\/\//i.test(u || '') ? esc(u) : '#';

  // Local state of one card: draft and the revision it was written against, the one operation in flight and its receipt.
  function fresh(row) { return {draft: '', bound: row.revision, op: null, receipt: null}; }

  // Free text the TaskQ `answer N --text` limits accept; null: sendable.
  function textProblem(q, text) {
    const lines = text.split('\n').filter(l => l.trim());
    if (!lines.length) return 'Type an answer';
    if (lines.length > q.limits.lines) return 'At most ' + q.limits.lines + ' lines';
    if (lines.some(l => [...l].length > q.limits.chars)) return 'At most ' + q.limits.chars + ' characters per line';
    return null;
  }

  const manageable = (row, cap) => row.state === 'ask' && cap && cap.answer && cap.manage && !(row.question && row.question.choices && row.question.choices.length);
  const draftQuestion = (row, cap) => manageable(row, cap) ? {limits: {lines: 1, chars: 232}} : row.question;

  // One of STATES for a row, its local state and the adapter's capability {answer, reason}.
  function status(row, local, cap) {
    const q = row.question;
    // a receipt belongs to the question it answered; one in flight or unknown holds across refreshes, an answered one
    // stays beside its task after the question is gone
    if (local.op && local.receipt && (['sending', 'unknown'].includes(local.receipt.state) || local.op.revision === row.revision || !q)) return local.receipt.state;
    if (!manageable(row, cap) && (!q || !q.parsed || !cap || !cap.answer || !['ask', 'review'].includes(row.state))) return 'read-only';
    if (local.bound !== row.revision && (local.draft.trim() || local.op || local.confirmClose)) return 'stale';
    return 'actionable';
  }

  // Pure transition: (local, event) -> {local, send}. `send` is an operation to hand to the adapter, else null.
  // Events: input {text}, choose {id}, submit, review, receipt {opId, state, reason, next}, refresh {row}, retry.
  function step(row, local, cap, ev, newId) {
    const st = status(row, local, cap), l = {...local};
    const start = payload => {
      l.confirmClose = false;
      l.op = {id: newId(), task: row.n, revision: row.revision, ...payload};
      l.bound = row.revision;
      l.receipt = {state: 'sending'};
      return {local: l, send: l.op};
    };
    switch (ev.type) {
      case 'input':
        // a draft typed while stale stays bound to the question it was written for
        if (st !== 'stale') l.bound = row.revision;
        l.draft = ev.text;
        return {local: l, send: null};
      case 'choose': {
        if (!['actionable', 'rejected'].includes(st)) return {local, send: null};
        const c = row.question.choices.find(x => x.id === ev.id);
        if (!c || c.consequential) return {local, send: null};
        return start({choice: c.id});
      }
      case 'submit':
        if (!cap || !cap.answer || !['actionable', 'rejected'].includes(st) || textProblem(draftQuestion(row, cap), l.draft)) return {local, send: null};
        return start(manageable(row, cap) ? {action: 'requeue', text: l.draft.trim()} : {text: l.draft.trim()});
      case 'later':
      case 'close':
      case 'confirm-close':
        if (!manageable(row, cap) || !['actionable', 'rejected'].includes(st)) return {local, send: null};
        if (ev.type === 'close') return {local: {...l, bound: row.revision, confirmClose: true}, send: null};
        if (ev.type === 'confirm-close' && !l.confirmClose) return {local, send: null};
        return start(ev.type === 'later' ? {action: 'later'} : {action: 'close', confirmed: true});
      case 'cancel-close':
        return {local: {...l, confirmClose: false}, send: null};
      case 'review':
        if (st === 'stale') { l.bound = row.revision; l.op = null; l.receipt = null; l.confirmClose = false; }
        return {local: l, send: null};
      case 'receipt':
        if (!l.op || ev.opId !== l.op.id) return {local, send: null};
        l.receipt = {state: ['accepted', 'rejected', 'stale', 'unknown'].includes(ev.state) ? ev.state : 'unknown', reason: ev.reason, next: ev.next};
        if (ev.state === 'accepted' && l.op.text) l.draft = '';
        return {local: l, send: null};
      case 'retry':
        // uncertain delivery reconciles with the same id and payload, never a new one
        return st === 'unknown' ? {local: {...l, receipt: {state: 'sending'}}, send: l.op} : {local, send: null};
      default:
        return {local, send: null};
    }
  }

  function render(board, row, local, cap) {
    const q = row.question || {text: row.card || '', parsed: false, choices: [], evidence: []}, st = status(row, local, cap);
    const id = 'qac-' + String(board).replace(/[^\w-]/g, '_') + '-' + row.n, live = st === 'actionable' || st === 'rejected';
    let h = '<section class="qac" data-qac-state="' + st + '" aria-labelledby="' + id + '-t">';
    h += '<p class="qac-head" id="' + id + '-t"><span class="qac-mark" aria-hidden="true">?</span> <a href="' + href(row.url) + '" target="_blank" rel="noopener">#' + esc(row.n) + '</a> ' + esc(row.title) + '</p>';
    if (manageable(row, cap)) {
      h += '<pre class="qac-text">' + esc(q.text || row.card || 'No question text supplied') + '</pre>';
      h += '<form class="qac-text-form" data-qac="submit"><textarea data-qac="input" rows="1" aria-label="Requeue reason" placeholder="Reason to requeue"' + (live ? '' : ' readonly') + '>' + esc(local.draft) + '</textarea><button type="submit"' + (live && !textProblem(draftQuestion(row, cap), local.draft) ? '' : ' disabled') + '>Requeue</button></form>';
      h += '<div class="qac-choices"><button type="button" data-qac="later"' + (live ? '' : ' disabled') + '>Later</button><button type="button" data-qac="close"' + (live ? '' : ' disabled') + '>Close…</button></div>';
      if (local.confirmClose && live) h += '<p>Cancel #' + esc(row.n) + ' without publication? Running work will stop. The task is parked before closing.</p><div class="qac-choices"><button type="button" data-qac="confirm-close">Close without publishing</button><button type="button" data-qac="cancel-close">Keep task</button></div>';
      h += receipt(st, local.receipt && local.receipt.reason, row.url, id, local.receipt && local.receipt.next, true);
      if (st === 'stale') h += '<button type="button" data-qac="review">I read the new question</button>';
      if (st === 'unknown') h += '<button type="button" data-qac="retry">Check delivery</button>';
      return h + '</section>';
    }
    if (!q.parsed) return h + '<pre class="qac-text">' + esc(q.text) + '</pre>' + receipt(st, st === 'read-only' ? 'This card has no answer form here' : local.receipt && local.receipt.reason, row.url, id) +
      (st === 'unknown' ? '<button type="button" data-qac="retry">Check delivery</button>' : '') + '</section>';
    h += '<p class="qac-problem">' + esc(q.problem) + '</p>';
    h += '<div class="qac-choices" role="group" aria-label="Choices">';
    for (const c of q.choices) {
      const off = !live || c.consequential, why = c.consequential ? 'Answer on the board: this choice closes or publishes the task' : '';
      h += '<button type="button" data-qac="choose" data-choice="' + c.id + '"' + (off ? ' disabled' : '') + (why ? ' aria-describedby="' + id + '-c' + c.id + '"' : '') + '><span class="qac-number">' + c.id + '.</span> <span>' + esc(c.text) +
        (c.recommended ? ' <span class="qac-star" role="img" aria-label="Recommended">★</span>' : '') + '</span></button>';
      if (why) h += '<span class="qac-why" id="' + id + '-c' + c.id + '">' + esc(why) + ' <a href="' + href(row.url) + '" target="_blank" rel="noopener">board ↗</a></span>';
    }
    h += '</div>';
    if (st !== 'read-only') {
      const bad = textProblem(q, local.draft);
      // Q1: Other answer opens a compact field; a kept draft keeps it open
      h += '<details class="qac-other"' + (local.draft ? ' open' : '') + '><summary>Other answer</summary><form class="qac-text-form" data-qac="submit"><textarea id="' + id + '-d" data-qac="input" rows="2" aria-label="Your answer" placeholder="Or type an answer"' + (live ? '' : ' readonly') + '>' + esc(local.draft) + '</textarea>' +
        '<button type="submit"' + (live && !bad ? '' : ' disabled') + '>Send</button></form></details>';
    }
    const reason = st === 'read-only' ? (cap && cap.reason) || 'Answering is not available here' : local.receipt && local.receipt.reason;
    h += receipt(st, reason, row.url, id, local.receipt && local.receipt.next);
    if (st === 'stale') h += '<button type="button" data-qac="review">I read the new question</button>';
    if (st === 'unknown') h += '<button type="button" data-qac="retry">Check delivery</button>';
    // Q1: Done and evidence stay behind Details
    if (q.done || q.evidence.length) h += '<details class="qac-context"><summary>Checked &amp; context</summary>' +
      (q.done ? '<p class="qac-done"><span class="qac-muted">Done:</span> ' + esc(q.done) + '</p>' : '') +
      (q.evidence.length ? '<p class="qac-evidence">' + q.evidence.map(e => '<a href="' + href(e.url) + '" target="_blank" rel="noopener">' + esc(e.label) + '</a>').join(' · ') + '</p>' : '') + '</details>';
    return h + '</section>';
  }

  function receipt(st, reason, url, id, next, action) {
    if (st === 'actionable') return '<p class="qac-receipt" id="' + id + '-r" role="status" aria-live="polite"></p>';
    return '<p class="qac-receipt qac-' + st + '" id="' + id + '-r" role="status" aria-live="polite">' + esc(st === 'accepted' && action ? 'Action accepted on the board' : LABELS[st]) + (reason ? ': ' + esc(reason) : '') +
      (next ? '. ' + esc(next) : '') + (st === 'read-only' || st === 'unknown' ? ' <a href="' + href(url) + '" target="_blank" rel="noopener">Open on board ↗</a>' : '') + '</p>';
  }

  // Browser binding: one container, rows by board. adapter = {capability(board, row) -> {answer, reason},
  // send(board, op) -> Promise<{state, reason, next}>, newId?() }. update() keeps drafts, focus and scroll.
  function mount(el, adapter) {
    const locals = new Map(), cards = new Map();
    const newId = adapter.newId || (() => (root.crypto && root.crypto.randomUUID ? root.crypto.randomUUID() : 'qac-' + Date.now().toString(36) + Math.random().toString(36).slice(2)));
    function apply(k, ev) {
      const {board, row} = cards.get(k), cap = adapter.capability(board, row), r = step(row, locals.get(k), cap, ev, newId);
      locals.set(k, r.local);
      if (ev.type !== 'input') paint();
      if (r.send) Promise.resolve().then(() => adapter.send(board, r.send))
        .then(x => apply(k, {type: 'receipt', opId: r.send.id, ...x}), e => apply(k, {type: 'receipt', opId: r.send.id, state: 'unknown', reason: String(e && e.message || e)}));
      else if (ev.type === 'input') {
        const s = el.querySelector('[data-qac-key="' + CSS.escape(k) + '"] button[type=submit]'), cap2 = adapter.capability(board, row);
        if (s) s.disabled = !!textProblem(draftQuestion(row, cap2), r.local.draft) || !['actionable', 'rejected'].includes(status(row, r.local, cap2));
      }
    }
    function paint() {
      const a = document.activeElement, focus = a && el.contains(a) ? {k: a.closest('[data-qac-key]')?.dataset.qacKey, sel: a.dataset.qac === 'input' ? 'textarea' : a.dataset.choice ? '[data-choice="' + a.dataset.choice + '"]' : '[data-qac="' + a.dataset.qac + '"]', s: a.selectionStart, e: a.selectionEnd} : null;
      const scroll = el.scrollTop, open = new Set([...el.querySelectorAll('details[open]')].map(d => d.closest('[data-qac-key]')?.dataset.qacKey + ' ' + d.className));
      el.innerHTML = [...cards].map(([k, c]) => '<div data-qac-key="' + esc(k) + '">' + render(c.board, c.row, locals.get(k), adapter.capability(c.board, c.row)) + '</div>').join('');
      for (const d of el.querySelectorAll('details')) if (open.has(d.closest('[data-qac-key]').dataset.qacKey + ' ' + d.className)) d.open = true;
      el.scrollTop = scroll;
      const t = focus && focus.k && el.querySelector('[data-qac-key="' + CSS.escape(focus.k) + '"] ' + focus.sel);
      if (t) { t.focus(); if (focus.s != null && t.setSelectionRange) t.setSelectionRange(focus.s, focus.e); }
    }
    el.addEventListener('click', e => {
      const b = e.target.closest('button[data-qac]'), k = b && b.closest('[data-qac-key]')?.dataset.qacKey;
      if (k && !b.disabled) apply(k, b.dataset.qac === 'choose' ? {type: 'choose', id: Number(b.dataset.choice)} : {type: b.dataset.qac});
    });
    el.addEventListener('submit', e => { e.preventDefault(); const k = e.target.closest('[data-qac-key]')?.dataset.qacKey; if (k) apply(k, {type: 'submit'}); });
    el.addEventListener('input', e => { const k = e.target.dataset.qac === 'input' && e.target.closest('[data-qac-key]')?.dataset.qacKey; if (k) apply(k, {type: 'input', text: e.target.value}); });
    return {
      // rows: [{board, row}] of open questions; a row that left keeps no state
      update(rows) {
        cards.clear();
        for (const c of rows) { const k = key(c.board, c.row); cards.set(k, c); if (!locals.has(k)) locals.set(k, fresh(c.row)); }
        for (const k of [...locals.keys()]) if (!cards.has(k)) locals.delete(k);
        paint();
      },
    };
  }

  // Conformance replay shared by every consumer harness: the fixture's events through `step` with the consumer's
  // adapter capability; returns each checkpoint's {status, html, sent} for the fixture's expectations.
  function replay(fx, capability) {
    let row = fx.rows[0], local = fresh(row), n = 0;
    const sent = [], out = [];
    for (const ev of fx.events || []) {
      if (ev.type === 'refresh') { row = fx.rows[ev.row]; continue; }
      if (ev.type === 'check') { out.push({status: status(row, local, capability(fx.board, row)), html: render(fx.board, row, local, capability(fx.board, row)), sent: sent.slice(), draft: local.draft}); continue; }
      const r = step(row, local, capability(fx.board, row), ev.type === 'receipt' ? {...ev, opId: sent[sent.length - 1] && sent[sent.length - 1].id} : ev, () => 'op-' + ++n);
      local = r.local;
      if (r.send) sent.push(r.send);
    }
    return out;
  }

  const api = {STATES, status, step, render, mount, replay, textProblem};
  if (typeof module === 'object' && module.exports) module.exports = api; else root.TaskqAnswerCard = api;
})(typeof globalThis === 'object' ? globalThis : this);
