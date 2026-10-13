// TaskQ shared resource controls (docs/design/shared-resource-controls.md). Source: https://github.com/taskqorg/taskq
// src/resource-controls.js, MIT, published with each TaskQ release as taskq-<version>-resource-controls.js and listed in
// SHA256SUMS; never edit a copy.
// Input: one node/project group of taskq.resources/1 Projection targets (host, project and runtime scopes) plus the
// adapter's labels and capability. No network client, config parser, admission rule or credentials: `send` is the adapter's.
(function (root) {
  'use strict';
  const STATES = ['view', 'editing', 'review', 'conflict', 'sending', 'saved', 'applied', 'rejected', 'unknown', 'read-only'];
  const FRESH = 60;
  const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]));
  const rtName = rt => ({claude: 'Claude', codex: 'Codex', 'codex-app': 'Codex app'}[rt] || rt);
  const BLOCKERS = {cpu: 'CPU', memory: 'memory', pressure: 'memory pressure', sample: 'no fresh sample', config: 'invalid config', cooldown: 'recovery',
    spacing: 'host spacing', max: 'host maximum', version: 'old service', host: 'host hold', runtime_disabled: 'runtime disabled', pause: 'paused',
    focus: 'focus', dependency: 'dependency', routing: 'routing', access: 'access', provider: 'provider limit', unknown: 'unknown reason'};
  const GATE = ['cpu', 'memory', 'pressure', 'cooldown'];

  // The group's scopes: host (no project), project (no runtime) and runtime targets, in runtime order.
  function scopes(group) {
    const t = group.targets || [];
    return {host: t.find(x => x.target.project_id == null), project: t.find(x => x.target.project_id != null && x.target.runtime_id == null),
      runtimes: t.filter(x => x.target.runtime_id != null)};
  }
  const tkey = t => [t.target.project_id, t.target.runtime_id].map(x => x == null ? '' : x).join('/');
  // Qualified Auto/Fixed authority versus the legacy Fixed guard; a consumer never infers it from a version string.
  const schemaOf = (x, s) => x && (x.supported || []).find(p => p.policy_schema === s);
  // The schema that advertises `field` at this target's scope; null: not editable here.
  const schemaFor = (x, field) => { const p = x && (x.supported || []).find(p => p.fields.includes(field) && p.scopes.includes(x.target.runtime_id != null ? 'runtime' : x.target.project_id != null ? 'project' : 'host')); return p ? p.policy_schema : null; };
  const editable = (x, field) => !!schemaFor(x, field);

  // Seconds since the producer's sample (unknown: null) and whether the observation itself is fresh.
  function freshness(group, now) {
    const p = scopes(group).project, obs = group.observed_at, s = p && p.sample;
    const status = obs == null || now < obs ? null : now - obs;
    const sample = s && s.sampled_at != null && s.age_seconds != null && status != null && s.sampled_at <= obs ? s.age_seconds + status : null;
    return {status, sample, fresh: status != null && status <= FRESH};
  }

  // Policy line: Auto/Fixed only under the qualified admission schema, else the legacy guard.
  function policy(group) {
    const h = scopes(group).host, c = h && h.current_policy || {};
    if (!schemaOf(h, 'taskq.admission/1')) return 'Legacy guard · Fixed limits';
    if (c.mode === 'auto') return 'Auto · ' + (c.maximum == null ? 'no host maximum' : 'host maximum ' + c.maximum);
    if (c.mode === 'fixed') return 'Fixed';
    return 'Policy unknown';
  }

  // Load gate text of taskq.md § 4 from the producer's blockers; unknown is never Off or Ready.
  function gate(group, now) {
    const p = scopes(group).project, f = freshness(group, now);
    if (!p) return 'Unknown: no capacity status';
    if (!f.fresh) return 'Unknown: status ' + (f.status == null ? 'missing' : age(f.status) + ' old');
    const g = p.effective_policy && p.effective_policy.load_gate, bs = (p.blockers || []);
    if (bs.some(b => b.code === 'config')) return 'Unknown: invalid config';
    if (g && g.enabled === false) return 'Off';
    if (!g) return 'Unknown: gate not reported';
    if (f.sample == null || f.sample > FRESH) return 'Unknown: ' + (f.sample == null ? 'no sample yet' : 'sample ' + age(f.sample) + ' old');
    if (bs.some(b => b.code === 'sample' || b.knowledge === 'unknown')) return 'Unknown: ' + bs.filter(b => b.code === 'sample' || b.knowledge === 'unknown').map(b => BLOCKERS[b.code] || BLOCKERS.unknown).join(', ');
    const held = bs.filter(b => GATE.includes(b.code));
    return held.length ? 'Waiting: ' + [...new Set(held.map(b => BLOCKERS[b.code]))].join(', ') : 'Ready';
  }

  function age(s) { return s < 120 ? s + ' s' : s < 7200 ? Math.round(s / 60) + ' min' : s < 172800 ? Math.round(s / 3600) + ' h' : Math.round(s / 86400) + ' d'; }

  // used/limit per runtime: unknown stays `?`, explicit zero is off, absent Fixed is none, Auto absent has no cap.
  function usage(group, now) {
    const p = scopes(group).project, fresh = freshness(group, now).fresh;
    return ((p && p.runtime_usage) || []).map(u => {
      const used = fresh && u.used != null ? u.used : null, l = u.limit;
      const limit = u.limit_kind === 'unbounded' ? 'no cap' : u.limit_kind === 'unknown' || (l == null && u.limit_kind !== 'disabled') ? '?' : l == null ? 'none' : String(l);
      const over = used != null && l != null && u.limit_kind !== 'unbounded' && used > l;
      return {runtime: u.runtime_id, used, limit, over, off: u.limit_kind === 'disabled',
        text: (used == null ? '?' : used) + '/' + limit + (over ? ' · over limit' : u.limit_kind === 'disabled' ? ' · off' : '')};
    });
  }

  // Saved versus applied for every target: applied needs a fresh observation whose effective revision equals the saved one.
  function applied(group, now) {
    const t = group.targets || [];
    if (!t.length || !freshness(group, now).fresh || t.some(x => x.effective_revision == null)) return 'unknown';
    return t.every(x => x.current_revision === x.effective_revision) ? 'applied' : 'saved';
  }

  function headline(group, now) { return policy(group) + ' · Load gate: ' + gate(group, now); }

  // Editor fields of the group in canonical order: runtime limits, then the project load gate.
  function fields(group) {
    const s = scopes(group), out = [];
    for (const r of s.runtimes) if (editable(r, 'runtime_limit')) out.push({key: 'rt:' + r.target.runtime_id, target: r, field: 'runtime_limit', label: rtName(r.target.runtime_id),
      nullable: schemaFor(r, 'runtime_limit') === 'taskq.admission/1', value: r.current_policy && r.current_policy.runtime_limit});
    if (editable(s.project, 'load_gate')) {
      const g = s.project.current_policy && s.project.current_policy.load_gate || {};
      out.push({key: 'gate.enabled', target: s.project, field: 'load_gate', label: 'Load gate', value: g.enabled, bool: true,
        locked: scopes(group).host && scopes(group).host.current_policy && scopes(group).host.current_policy.mode === 'auto'});
      out.push({key: 'gate.cpu_max', target: s.project, field: 'load_gate', label: 'CPU ceiling %', value: g.cpu_max, lo: 10, hi: 100});
      out.push({key: 'gate.memory_min', target: s.project, field: 'load_gate', label: 'Memory floor %', value: g.memory_min, lo: 1, hi: 95});
    }
    return out;
  }
  const shown = f => f.bool ? !!f.value : f.value == null ? '' : String(f.value);
  const said = f => f.bool ? (f.value ? 'on' : 'off') : f.value == null ? 'none' : String(f.value);

  // Explicitly edited fields that differ from the saved policy; untouched fields never travel.
  function edited(group, d) { return fields(group).filter(f => d.touched[f.key] && d.values[f.key] !== shown(f)); }

  function problem(group, d) {
    const n = (v, lo, hi) => /^\d+$/.test(v) && Number(v) >= lo && Number(v) <= hi;
    for (const f of edited(group, d)) {
      const v = d.values[f.key];
      if (f.bool) { if (f.locked && !v) return 'Auto keeps the load gate on'; continue; }
      if (f.field === 'runtime_limit' && v === '') { if (!f.nullable) return f.label + ': Fixed limits need a number; 0 admits none'; continue; }
      if (f.field === 'runtime_limit' && !n(v, 0, 4294967295)) return f.label + ': an integer 0 or more';
      if (f.lo != null && !n(v, f.lo, f.hi)) return f.label + ': an integer ' + f.lo + '..' + f.hi;
    }
    return edited(group, d).length ? null : 'No change';
  }

  // One operation per target, contract-shaped (taskq.resources/1 Changes); load_gate travels whole.
  function changes(group, d) {
    const by = new Map();
    for (const f of edited(group, d)) {
      const k = tkey(f.target);
      if (!by.has(k)) by.set(k, {target: f.target.target, policy_schema: schemaFor(f.target, f.field),
        expected_revision: f.target.current_revision, changes: {}});
      const c = by.get(k).changes;
      if (f.field === 'runtime_limit') c.runtime_limit = d.values[f.key] === '' ? null : Number(d.values[f.key]);
      else {
        const g = fields(group).filter(x => x.field === 'load_gate'), v = x => d.touched[x.key] ? d.values[x.key] : shown(x);
        c.load_gate = {enabled: !!v(g[0]), cpu_max: Number(v(g[1])), memory_min: Number(v(g[2]))};
      }
    }
    return [...by.values()];
  }

  // Review lines: exact old → new per edited field.
  function review(group, d) {
    return edited(group, d).map(f => f.label + ' ' + said(f) + ' → ' + (f.bool ? (d.values[f.key] ? 'on' : 'off') : d.values[f.key] === '' ? 'none' : d.values[f.key]));
  }

  // Local state: an editor draft bound to the revisions it was opened against, one operation and its receipt.
  function fresh() { return {draft: null, op: null, receipt: null}; }
  const revs = group => Object.fromEntries((group.targets || []).map(t => [tkey(t), t.current_revision]));

  function status(group, local, cap, now) {
    if (local.receipt && local.receipt.state === 'saved') return applied(group, now) === 'applied' && JSON.stringify(revs(group)) !== JSON.stringify(local.receipt.from) ? 'applied' : 'saved';
    if (local.receipt) return local.receipt.state;
    if (!local.draft) return cap && cap.edit && fields(group).length ? 'view' : 'read-only';
    if (JSON.stringify(local.draft.base) !== JSON.stringify(revs(group))) return 'conflict';
    return local.draft.review ? 'review' : 'editing';
  }

  // Pure transition: (local, event) -> {local, send}. Events: open, input {key, value}, review, back, cancel, confirm,
  // rebase, receipt {opId, state, reason}, retry, dismiss.
  function step(group, local, cap, ev, newId, now) {
    const st = status(group, local, cap, now), l = {...local};
    switch (ev.type) {
      case 'open':
        if (!['view', 'applied', 'rejected'].includes(st)) return {local, send: null};
        return {local: {draft: {base: revs(group), values: Object.fromEntries(fields(group).map(f => [f.key, shown(f)])), touched: {}, review: false, error: null}, op: null, receipt: null}, send: null};
      case 'input':
        if (!l.draft || ['sending'].includes(st)) return {local, send: null};
        l.draft = {...l.draft, values: {...l.draft.values, [ev.key]: ev.value}, touched: {...l.draft.touched, [ev.key]: true}, review: false, error: null};
        return {local: l, send: null};
      case 'review':
        if (st !== 'editing') return {local, send: null};
        l.draft = {...l.draft, error: problem(group, l.draft)};
        l.draft.review = !l.draft.error;
        return {local: l, send: null};
      case 'back':
        if (st === 'review') l.draft = {...l.draft, review: false};
        return {local: l, send: null};
      case 'rebase':
        // a conflict keeps the typed values, binds them to the new revisions and asks for a fresh review
        if (st === 'conflict') l.draft = {...l.draft, base: revs(group), review: false};
        return {local: l, send: null};
      case 'cancel':
        return ['editing', 'review', 'conflict'].includes(st) ? {local: fresh(), send: null} : {local, send: null};
      case 'confirm': {
        if (st !== 'review') return {local, send: null};
        l.op = {id: newId(), ops: changes(group, l.draft)};
        l.receipt = {state: 'sending', from: revs(group)};
        l.draft = null;
        return {local: l, send: l.op};
      }
      case 'receipt':
        if (!l.op || ev.opId !== l.op.id) return {local, send: null};
        l.receipt = {...l.receipt, state: ['saved', 'rejected', 'unknown'].includes(ev.state) ? ev.state : 'unknown', reason: ev.reason};
        return {local: l, send: null};
      case 'retry':
        // uncertain delivery reconciles with the same id and payload, never a new operation
        return st === 'unknown' ? {local: {...l, receipt: {...l.receipt, state: 'sending', reason: null}}, send: l.op} : {local, send: null};
      case 'dismiss':
        return ['applied', 'rejected'].includes(st) ? {local: fresh(), send: null} : {local, send: null};
      default:
        return {local, send: null};
    }
  }

  const RECEIPT = {sending: 'Saving…', saved: 'Saved; waiting for service', applied: 'Applied: read back from the service',
    rejected: 'Not saved', unknown: 'Delivery unknown: check again', conflict: 'Settings changed since you opened the editor'};

  function render(group, local, cap, now) {
    const st = status(group, local, cap, now), f = freshness(group, now), s = scopes(group), id = 'rsc-' + String(group.key || '').replace(/[^\w-]/g, '_');
    let h = '<section class="rsc" data-rsc-state="' + st + '" aria-label="' + esc('Capacity of ' + (group.labels && group.labels.project || 'project') + ' on ' + (group.labels && group.labels.host || 'this host')) + '">';
    h += '<p class="rsc-row"><span>Policy</span><span class="rsc-muted">' + esc(policy(group)) + '</span></p>';
    for (const u of usage(group, now)) h += '<p class="rsc-row' + (u.over ? ' rsc-over' : '') + '" aria-label="' + esc(rtName(u.runtime) + ': ' + (u.used == null ? 'unknown' : u.used) + ' used of ' + u.limit + (u.over ? ', over limit; running tasks continue' : '')) + '"><span>' + esc(rtName(u.runtime)) + '</span><span class="rsc-count">' + esc(u.text) + '</span></p>';
    h += '<p class="rsc-row"><span>Load gate</span><span class="rsc-muted">' + esc(gate(group, now)) + '</span></p>';
    const sm = s.project && s.project.sample;
    if (sm && (sm.cpu_busy_percent != null || sm.memory_available_percent != null)) {
      const parts = [sm.cpu_busy_percent != null ? 'CPU ' + Math.round(sm.cpu_busy_percent) + ' %' : 'CPU unknown', sm.memory_available_percent != null ? 'available memory ' + Math.round(sm.memory_available_percent) + ' %' : 'memory unknown'];
      h += '<p class="rsc-muted">' + esc(parts.join(' · ') + (f.sample != null ? ' · ' + (f.sample > FRESH ? 'last sample ' + age(f.sample) + ' ago' : age(f.sample) + ' ago') : '')) + '</p>';
    }
    const other = (s.project && s.project.blockers || []).filter(b => !GATE.includes(b.code) && b.code !== 'sample' && b.code !== 'config');
    if (other.length) h += '<p class="rsc-muted">Also blocked: ' + esc(other.map(b => BLOCKERS[b.code] || BLOCKERS.unknown).join(', ')) + '</p>';
    if (s.project && s.project.completeness !== 'complete') h += '<p class="rsc-muted">Blockers ' + esc(s.project.completeness === 'partial' ? 'partly unknown' : 'unknown') + '</p>';
    if (cap && cap.note) h += '<p class="rsc-alert">' + esc(cap.note) + '</p>';
    if (st === 'view' || st === 'read-only') {
      const a = applied(group, now);
      if (a === 'saved') h += '<p class="rsc-muted" role="status">' + RECEIPT.saved + '</p>';
      return h + (st === 'view' ? '<button type="button" data-rsc="open">Edit limits</button>' : cap && cap.reason ? '<p class="rsc-muted">' + esc(cap.reason) + '</p>' : '') + '</section>';
    }
    if (['sending', 'saved', 'applied', 'rejected', 'unknown'].includes(st)) {
      const r = local.receipt;
      h += '<p class="rsc-receipt rsc-' + st + '" role="status" aria-live="polite">' + esc(RECEIPT[st] + (r && r.reason ? ': ' + r.reason : '')) + '</p>';
      if (st === 'unknown') h += '<button type="button" data-rsc="retry">Check again</button>';
      if (st === 'applied' || st === 'rejected') h += '<button type="button" data-rsc="open">Edit limits</button>';
      return h + '</section>';
    }
    const d = local.draft;
    if (st === 'review') return h + '<div class="rsc-review" role="alert"><p>Apply to ' + esc((group.labels && group.labels.project) || 'project') + ' on ' + esc((group.labels && group.labels.host) || 'this host') + '?</p>' +
      review(group, d).map(x => '<p>' + esc(x) + '</p>').join('') + '<p class="rsc-muted">Running tasks continue. Other hosts and unchanged settings stay.</p></div>' +
      '<div class="rsc-actions"><button type="button" data-rsc="confirm">Confirm</button><button type="button" data-rsc="back">Back</button><button type="button" data-rsc="cancel">Cancel</button></div></section>';
    h += '<div class="rsc-form">';
    for (const x of fields(group)) {
      const fid = id + '-' + x.key.replace(/[^\w-]/g, '_'), v = d.values[x.key];
      h += '<label for="' + fid + '">' + esc(x.label) + '</label>';
      h += x.bool ? '<input id="' + fid + '" type="checkbox" role="switch" data-rsc-field="' + esc(x.key) + '"' + (v ? ' checked' : '') + (x.locked ? ' disabled aria-describedby="' + fid + '-why"' : '') + '>' + (x.locked ? '<span class="rsc-why" id="' + fid + '-why">Auto keeps the load gate on</span>' : '')
        : '<input id="' + fid + '" type="number" inputmode="numeric" step="1" min="0" data-rsc-field="' + esc(x.key) + '" value="' + esc(v) + '"' + (x.value == null ? ' placeholder="none"' : '') + '>';
    }
    h += '</div>';
    if (st === 'conflict') h += '<p class="rsc-alert" role="alert">' + RECEIPT.conflict + '</p>';
    if (d.error) h += '<p class="rsc-alert" role="alert">' + esc(d.error) + '</p>';
    return h + '<div class="rsc-actions">' + (st === 'conflict' ? '<button type="button" data-rsc="rebase">Review new values</button>' : '<button type="button" data-rsc="review">Apply…</button>') +
      '<button type="button" data-rsc="cancel">Cancel</button></div></section>';
  }

  // Browser binding: one element per group. adapter = {capability(group) -> {edit, reason, note}, send(group, op) ->
  // Promise<{state, reason}>, now() -> seconds, newId?()}. update(group) keeps the draft, focus and caret.
  function mount(el, adapter) {
    let group = null, local = fresh();
    const now = () => adapter.now ? adapter.now() : Math.floor(Date.now() / 1000);
    const newId = adapter.newId || (() => (root.crypto && root.crypto.randomUUID ? root.crypto.randomUUID() : 'rsc-' + Date.now().toString(36) + Math.random().toString(36).slice(2)));
    function apply(ev) {
      const r = step(group, local, adapter.capability(group), ev, newId, now());
      local = r.local;
      if (ev.type !== 'input') paint();
      if (r.send) Promise.resolve().then(() => adapter.send(group, r.send))
        .then(x => apply({type: 'receipt', opId: r.send.id, ...x}), e => apply({type: 'receipt', opId: r.send.id, state: 'unknown', reason: String(e && e.message || e)}));
    }
    function paint() {
      const a = document.activeElement, focus = a && el.contains(a) ? {sel: a.dataset.rscField ? '[data-rsc-field="' + a.dataset.rscField + '"]' : '[data-rsc="' + a.dataset.rsc + '"]', s: a.type === 'number' ? null : a.selectionStart, e: a.selectionEnd} : null;
      el.innerHTML = render(group, local, adapter.capability(group), now());
      const t = focus && el.querySelector(focus.sel);
      if (t) { t.focus(); if (focus.s != null && t.setSelectionRange) try { t.setSelectionRange(focus.s, focus.e); } catch (_) { /* number inputs */ } }
    }
    el.addEventListener('click', e => { const b = e.target.closest('button[data-rsc]'); if (b && !b.disabled) apply({type: b.dataset.rsc}); });
    el.addEventListener('input', e => { const k = e.target.dataset.rscField; if (k) apply({type: 'input', key: k, value: e.target.type === 'checkbox' ? e.target.checked : e.target.value}); });
    return {
      update(g) { group = g; paint(); },
      editing() { return !!local.draft; },
    };
  }

  // Conformance replay shared by every consumer harness: the fixture's events through `step`; each check returns
  // {status, html, sent, headline}. Refresh {group} replaces the observed group; now comes from the case.
  function replay(fx, capability) {
    let group = fx.groups[0], local = fresh(), n = 0;
    const sent = [], out = [], cap = g => capability(g) || fx.capability;
    for (const ev of fx.events || []) {
      if (ev.type === 'refresh') { group = fx.groups[ev.group]; continue; }
      if (ev.type === 'check') { out.push({status: status(group, local, cap(group), fx.now), html: render(group, local, cap(group), fx.now), sent: sent.slice(), headline: headline(group, fx.now)}); continue; }
      const r = step(group, local, cap(group), ev.type === 'receipt' ? {...ev, opId: sent.length && sent[sent.length - 1].id} : ev, () => 'op-' + ++n, fx.now);
      local = r.local;
      if (r.send) sent.push(r.send);
    }
    return out;
  }

  const api = {STATES, scopes, freshness, policy, gate, usage, applied, headline, fields, changes, review, status, step, render, mount, replay};
  if (typeof module === 'object' && module.exports) module.exports = api; else root.TaskqResources = api;
})(typeof globalThis === 'object' ? globalThis : this);
