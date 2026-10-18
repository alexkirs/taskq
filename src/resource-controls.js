'use strict';
const ui=require('./ui-runtime.js');
const execution=ui.execution;

const {scopes,freshness,policy,gate,age,usage,applied,headline,fields,edited,problem,changes,review,fresh,status,step,render,input,mount,workFields,freshWork,workStatus,workStep,workRender,mountWork,replayWork,replay}=execution.module("resource-controls",{
scopes:function(group) {
    const t = group.targets || [];
    const host=t.find(x => x.target.project_id == null && x.target.runtime_id == null),project=t.find(x => x.target.project_id != null && x.target.runtime_id == null);
    return {host, project, observation:schemaOf(host, 'taskq.host-capacity/1')?host:project, runtimes:t.filter(x => x.target.runtime_id != null)};
  },
freshness:function(group, now) {
    const p = scopes(group).observation, obs = group.observed_at, s = p && p.sample;
    const status = obs == null || now < obs ? null : now - obs;
    const sample = s && s.sampled_at != null && s.age_seconds != null && status != null && s.sampled_at <= obs ? s.age_seconds + status : null;
    return {status, sample, fresh: status != null && status <= FRESH};
  },
policy:function(group) {
    const h = scopes(group).host, c = h && h.current_policy || {};
    if (schemaOf(h, 'taskq.host-capacity/1')) return 'Host limits · ' + (c.maximum == null ? 'no maximum' : 'maximum ' + c.maximum);
    if (!schemaOf(h, 'taskq.admission/1')) return 'Legacy guard · Fixed limits';
    if (c.mode === 'auto') return 'Auto · ' + (c.maximum == null ? 'no host maximum' : 'host maximum ' + c.maximum);
    if (c.mode === 'fixed') return 'Fixed';
    return 'Policy unknown';
  },
gate:function(group, now) {
    const p = scopes(group).observation, f = freshness(group, now);
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
  },
age:function(s) { return s < 120 ? s + ' s' : s < 7200 ? Math.round(s / 60) + ' min' : s < 172800 ? Math.round(s / 3600) + ' h' : Math.round(s / 86400) + ' d'; },
usage:function(group, now) {
    const p = scopes(group).observation, fresh = freshness(group, now).fresh;
    return ((p && p.runtime_usage) || []).map(u => {
      const used = fresh && u.used != null ? u.used : null, l = u.limit;
      const limit = u.limit_kind === 'unbounded' ? 'no cap' : u.limit_kind === 'unknown' || (l == null && u.limit_kind !== 'disabled') ? '?' : l == null ? 'none' : String(l);
      const over = used != null && l != null && u.limit_kind !== 'unbounded' && used > l;
      return {runtime: u.runtime_id, used, limit, over, off: u.limit_kind === 'disabled',
        text: (used == null ? '?' : used) + '/' + limit + (over ? ' · over limit' : u.limit_kind === 'disabled' ? ' · off' : '')};
    });
  },
applied:function(group, now) {
    const t = group.targets || [];
    if (!t.length || !freshness(group, now).fresh || t.some(x => x.effective_revision == null)) return 'unknown';
    return t.every(x => x.current_revision === x.effective_revision) ? 'applied' : 'saved';
  },
headline:function(group, now) { return policy(group) + ' · Load gate: ' + gate(group, now); },
fields:function(group) {
    const s = scopes(group), out = [], hc = s.host && s.host.current_policy || {};
    if (editable(s.host, 'mode')) out.push({key: 'host.mode', target: s.host, field: 'mode', label: 'Mode', value: hc.mode, choice: ['auto', 'fixed']});
    if (editable(s.host, 'maximum')) out.push({key: 'host.maximum', target: s.host, field: 'maximum', label: 'Host maximum', nullable: true, value: hc.maximum, none: 'no maximum'});
    for (const r of s.runtimes) if (editable(r, 'runtime_limit')) out.push({key: 'rt:' + r.target.runtime_id, target: r, field: 'runtime_limit', label: rtName(r.target.runtime_id),
      nullable: ['taskq.admission/1','taskq.host-capacity/1'].includes(schemaFor(r, 'runtime_limit')),none:schemaFor(r,'runtime_limit')==='taskq.host-capacity/1'?'no cap':null, value: r.current_policy && r.current_policy.runtime_limit});
    const target=editable(s.host,'load_gate')?s.host:s.project;
    if (editable(target, 'load_gate')) {
      const g = target.current_policy && target.current_policy.load_gate || {};
      out.push({key: 'gate.enabled', target, field: 'load_gate', label: 'Load gate', value: g.enabled, bool: true,
        locked: scopes(group).host && scopes(group).host.current_policy && scopes(group).host.current_policy.mode === 'auto'});
      out.push({key: 'gate.cpu_max', target, field: 'load_gate', label: 'CPU ceiling %', value: g.cpu_max, lo: 10, hi: 100});
      out.push({key: 'gate.memory_min', target, field: 'load_gate', label: 'Memory floor %', value: g.memory_min, lo: 1, hi: 95});
    }
    return out;
  },
edited:function(group, d) { return fields(group).filter(f => d.touched[f.key] && d.values[f.key] !== shown(f)); },
problem:function(group, d) {
    const {n}=execution.module("resource-controls.problem",{n:(v, lo, hi) => /^\d+$/.test(v) && Number(v) >= lo && Number(v) <= hi});
    for (const f of edited(group, d)) {
      const v = d.values[f.key];
      if (f.bool) { if (f.locked && !v && d.values['host.mode'] !== 'fixed') return 'Auto keeps the load gate on'; continue; }
      if (f.choice) { if (!f.choice.includes(v)) return f.label + ': Auto or Fixed'; continue; }
      if (f.field === 'maximum' && v !== '' && !n(v, 0, 4294967295)) return f.label + ': an integer 0 or more, or empty for no maximum';
      if (f.field === 'runtime_limit' && v === '') { if (!f.nullable) return f.label + ': Fixed limits need a number; 0 admits none'; continue; }
      if (f.field === 'runtime_limit' && !n(v, 0, 4294967295)) return f.label + ': an integer 0 or more';
      if (f.lo != null && !n(v, f.lo, f.hi)) return f.label + ': an integer ' + f.lo + '..' + f.hi;
    }
    if (!edited(group, d).length) return 'No change';
    // Auto cannot run with the gate off (taskq.md § 9): check the resulting pair, edited or not
    const {now}=execution.module("resource-controls.problem",{now:k => { const f = fields(group).find(x => x.key === k); return f && (d.touched[k] ? d.values[k] : shown(f)); }});
    return now('host.mode') === 'auto' && now('gate.enabled') === false ? 'Auto keeps the load gate on' : null;
  },
changes:function(group, d) {
    const by = new Map();
    for (const f of edited(group, d)) {
      const k = tkey(f.target);
      if (!by.has(k)) by.set(k, {target: f.target.target, policy_schema: schemaFor(f.target, f.field),
        expected_revision: f.target.current_revision, changes: {}});
      const c = by.get(k).changes;
      if (f.field === 'runtime_limit') c.runtime_limit = d.values[f.key] === '' ? null : Number(d.values[f.key]);
      else if (f.field === 'mode') c.mode = d.values[f.key];
      else if (f.field === 'maximum') c.maximum = d.values[f.key] === '' ? null : Number(d.values[f.key]);
      else {
        const g = fields(group).filter(x => x.field === 'load_gate');
const {v}=execution.module("resource-controls.changes.helpers",{v:x => d.touched[x.key] ? d.values[x.key] : shown(x)});
        c.load_gate = {enabled: !!v(g[0]), cpu_max: Number(v(g[1])), memory_min: Number(v(g[2]))};
      }
    }
    return [...by.values()];
  },
review:function(group, d) {
    return edited(group, d).map(f => f.label + ' ' + said(f) + ' → ' + word(f, d.values[f.key]));
  },
fresh:function() { return {draft: null, op: null, receipt: null}; },
status:function(group, local, cap, now) {
    if(local.receipt?.state==='saved'&&schemaOf(scopes(group).host,'taskq.host-capacity/1')){
      const revision=local.receipt.saved_revision;
      if(typeof revision!=='string')return 'unknown';
      const targets=(local.op?.ops||[]).map(op=>({op,target:(group.targets||[]).find(t=>tkey(t)===tkey({target:op.target}))}));
      if(!targets.length||targets.some(x=>!x.target)||!freshness(group,now).fresh)return 'saved';
      if(targets.every(x=>x.target.current_revision===revision&&x.target.effective_revision===revision))return 'applied';
      return targets.some(x=>![revision,x.op.expected_revision].includes(x.target.current_revision))?'superseded':'saved';
    }
    if (local.receipt && local.receipt.state === 'saved') return applied(group, now) === 'applied' && JSON.stringify(revs(group)) !== JSON.stringify(local.receipt.from) ? 'applied' : 'saved';
    if (local.receipt) return local.receipt.state;
    if (!local.draft) return cap && cap.edit && fields(group).length ? 'view' : 'read-only';
    if (JSON.stringify(local.draft.base) !== JSON.stringify(revs(group))) return 'conflict';
    return local.draft.review ? 'review' : 'editing';
  },
step:function(group, local, cap, ev, newId, now) {
    const st = status(group, local, cap, now), l = {...local};
    switch (ev.type) {
      case 'open':
        if (!['view', 'applied', 'rejected','superseded'].includes(st)) return {local, send: null};
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
        l.receipt = {...l.receipt, state: ['saved', 'rejected', 'unknown'].includes(ev.state) ? ev.state : 'unknown', reason: ev.reason,saved_revision:ev.saved_revision};
        return {local: l, send: null};
      case 'retry':
        // uncertain delivery reconciles with the same id and payload, never a new operation
        return st === 'unknown' ? {local: {...l, receipt: {...l.receipt, state: 'sending', reason: null}}, send: l.op} : {local, send: null};
      case 'dismiss':
        return ['applied', 'rejected','superseded'].includes(st) ? {local: fresh(), send: null} : {local, send: null};
      default:
        return {local, send: null};
    }
  },
render:function(group, local, cap, now) {
    const st = status(group, local, cap, now), f = freshness(group, now), s = scopes(group), id = 'rsc-' + String(group.key || '').replace(/[^\w-]/g, '_');
    let h = '<section class="rsc" data-rsc-state="' + st + '" aria-label="' + esc('Capacity of ' + (schemaOf(s.host,'taskq.host-capacity/1')?'all registered projects':group.labels?.project||'project') + ' on ' + (group.labels?.host||'this host')) + '">';
    h += '<p class="rsc-row"><span>Policy</span><span class="rsc-muted">' + esc(policy(group)) + '</span></p>';
    for (const u of usage(group, now)) h += '<p class="rsc-row' + (u.over ? ' rsc-over' : '') + '" aria-label="' + esc(rtName(u.runtime) + ': ' + (u.used == null ? 'unknown' : u.used) + ' used of ' + u.limit + (u.over ? ', over limit; running tasks continue' : '')) + '"><span>' + esc(rtName(u.runtime)) + '</span><span class="rsc-count">' + esc(u.text) + '</span></p>';
    h += '<p class="rsc-row"><span>Load gate</span><span class="rsc-muted">' + esc(gate(group, now)) + '</span></p>';
    const sm = s.observation && s.observation.sample;
    if (sm && (sm.cpu_busy_percent != null || sm.memory_available_percent != null)) {
      const parts = [sm.cpu_busy_percent != null ? 'CPU ' + Math.round(sm.cpu_busy_percent) + ' %' : 'CPU unknown', sm.memory_available_percent != null ? 'available memory ' + Math.round(sm.memory_available_percent) + ' %' : 'memory unknown'];
      h += '<p class="rsc-muted">' + esc(parts.join(' · ') + (f.sample != null ? ' · ' + (f.sample > FRESH ? 'last sample ' + age(f.sample) + ' ago' : age(f.sample) + ' ago') : '')) + '</p>';
    }
    const other = (s.observation && s.observation.blockers || []).filter(b => !GATE.includes(b.code) && b.code !== 'sample' && b.code !== 'config');
    if (other.length) h += '<p class="rsc-muted">Also blocked: ' + esc(other.map(b => BLOCKERS[b.code] || BLOCKERS.unknown).join(', ')) + '</p>';
    if (s.observation && s.observation.completeness !== 'complete') h += '<p class="rsc-muted">Blockers ' + esc(s.observation.completeness === 'partial' ? 'partly unknown' : 'unknown') + '</p>';
    if (schemaOf(s.host, 'taskq.host-capacity/1')) {
      const limits=s.project?.current_policy?.limits;
      h += '<p class="rsc-muted">Project ceilings ('+esc(group.labels?.project||'project')+'): ' + esc(limits==null?'unknown':Object.entries(limits).map(([rt,n])=>rtName(rt)+' '+n).join(', ')||'none') + '</p>';
    }
    if (cap && cap.note) h += '<p class="rsc-alert">' + esc(cap.note) + '</p>';
    if (st === 'view' || st === 'read-only') {
      const a = applied(group, now);
      if (a === 'saved') h += '<p class="rsc-muted" role="status">' + RECEIPT.saved + '</p>';
      return h + (st === 'view' ? '<button type="button" data-rsc="open">Edit limits</button>' : cap && cap.reason ? '<p class="rsc-muted">' + esc(cap.reason) + '</p>' : '') + '</section>';
    }
    if (['sending', 'saved', 'applied', 'rejected', 'unknown','superseded'].includes(st)) {
      const r = local.receipt;
      h += '<p class="rsc-receipt rsc-' + st + '" role="status" aria-live="polite">' + esc(RECEIPT[st] + (r && r.reason ? ': ' + r.reason : '')) + '</p>';
      if (st === 'unknown') h += '<button type="button" data-rsc="retry">Check again</button>';
      if (st === 'applied' || st === 'rejected'||st==='superseded') h += '<button type="button" data-rsc="open">Edit limits</button>';
      return h + '</section>';
    }
    const d = local.draft;
    if (st === 'review') return h + '<div class="rsc-review" role="alert"><p>'+(schemaOf(s.host,'taskq.host-capacity/1')?'Apply host limits to all registered projects on '+esc(group.labels?.host||'this host'):'Apply to '+esc(group.labels?.project||'project')+' on '+esc(group.labels?.host||'this host'))+'?</p>' +
      review(group, d).map(x => '<p>' + esc(x) + '</p>').join('') + '<p class="rsc-muted">Running tasks continue. Other hosts and unchanged settings stay.</p></div>' +
      '<div class="rsc-actions"><button type="button" data-rsc="confirm">Confirm</button><button type="button" data-rsc="back">Back</button><button type="button" data-rsc="cancel">Cancel</button></div></section>';
    h += '<div class="rsc-form">' + fields(group).map(x => input(id, x, d.values[x.key])).join('') + '</div>';
    if (st === 'conflict') h += '<p class="rsc-alert" role="alert">' + RECEIPT.conflict + '</p>';
    if (d.error) h += '<p class="rsc-alert" role="alert">' + esc(d.error) + '</p>';
    return h + '<div class="rsc-actions">' + (st === 'conflict' ? '<button type="button" data-rsc="rebase">Review new values</button>' : '<button type="button" data-rsc="review">Apply…</button>') +
      '<button type="button" data-rsc="cancel">Cancel</button></div></section>';
  },
input:function(id, x, v, mixed, off) {
    const fid = id + '-' + x.key.replace(/[^\w-]/g, '_'), dis = off ? ' disabled' : '';
    let h = '<label for="' + fid + '">' + esc(x.label) + '</label>';
    if (x.choice) return h + '<select id="' + fid + '" data-rsc-field="' + esc(x.key) + '"' + dis + '>' + (mixed ? '<option value=""' + (v === '' ? ' selected' : '') + '>Mixed</option>' : '') +
      x.choice.map(c => '<option value="' + c + '"' + (v === c ? ' selected' : '') + '>' + MODES[c] + '</option>').join('') + '</select>';
    if (x.bool) return h + '<input id="' + fid + '" type="checkbox" role="switch" data-rsc-field="' + esc(x.key) + '"' + (v ? ' checked' : '') + (x.locked || off ? ' disabled' : '') + (x.locked ? ' aria-describedby="' + fid + '-why"' : '') + (mixed ? ' aria-label="' + esc(x.label) + ': Mixed"' : '') + '>' +
      (x.locked ? '<span class="rsc-why" id="' + fid + '-why">Auto keeps the load gate on</span>' : mixed ? '<span class="rsc-why">Mixed</span>' : '');
    return h + '<input id="' + fid + '" type="number" inputmode="numeric" step="1" min="0" data-rsc-field="' + esc(x.key) + '" value="' + esc(v) + '"' + dis + (mixed ? ' placeholder="Mixed"' : x.value == null ? ' placeholder="' + esc(x.none || 'none') + '"' : '') + '>';
  },
mount:function(el, adapter) {
const {apply,paint}=execution.module("resource-controls.mount",{
apply:function(ev) {
      const r = step(group, local, adapter.capability(group), ev, newId, now());
      local = r.local;
      if (ev.type !== 'input') paint();
      if (r.send) ui.effect('resource.send',()=>adapter.send(group,r.send),x=>apply({type:'receipt',opId:r.send.id,...x}));
    },
paint:function() {
      const a = document.activeElement, focus = a && el.contains(a) ? {sel: a.dataset.rscField ? '[data-rsc-field="' + a.dataset.rscField + '"]' : '[data-rsc="' + a.dataset.rsc + '"]', s: a.type === 'number' ? null : a.selectionStart, e: a.selectionEnd} : null;
      el.innerHTML = render(group, local, adapter.capability(group), now());
      const t = focus && el.querySelector(focus.sel);
      if (t) { t.focus(); if (focus.s != null && t.setSelectionRange) try { t.setSelectionRange(focus.s, focus.e); } catch (_) { /* number inputs */ } }
    }
});

    let group = null, local = fresh();
    const {now}=execution.module("resource-controls.mount",{now:() => adapter.now ? adapter.now() : Math.floor(Date.now() / 1000)});
    const newId = adapter.newId || (() => (root.crypto && root.crypto.randomUUID ? root.crypto.randomUUID() : 'rsc-' + Date.now().toString(36) + Math.random().toString(36).slice(2)));


    ui.listen(el,'click', e => { const b = e.target.closest('button[data-rsc]'); if (b && !b.disabled) apply({type: b.dataset.rsc}); });
    ui.listen(el,'input', e => { const k = e.target.dataset.rscField; if (k) apply({type: 'input', key: k, value: e.target.type === 'checkbox' ? e.target.checked : e.target.value}); });
    return execution.module("resource-controls.mount.controller",{
      update(g) { group = g; paint(); },
      editing() { return !!local.draft; },
    });
  },
workFields:function(groups) {
    const by = new Map();
    for (const g of groups) for (const f of fields(g)) {
      const x = by.get(f.key) || {...f, values: [], locked: false};
      x.values.push(shown(f));
      x.locked = x.locked || !!f.locked;
      by.set(f.key, x);
    }
    return [...by.values()].map(f => ({...f, mixed: new Set(f.values.map(String)).size > 1 || f.values.length < groups.length}))
      .sort((a, b) => RANK(a.key) - RANK(b.key));
  },
freshWork:function(selected) { return {selected: {...selected}, draft: null, locals: {}}; },
workStatus:function(groups, w, cap, now) {
    const sel = picked(groups, w), sent = sel.filter(g => w.locals[g.key]);
    if (sent.length) return {state: 'receipts', each: Object.fromEntries(sent.map(g => [g.key, status(g, w.locals[g.key], cap(g), now)]))};
    if(sel.some(g=>schemaOf(scopes(g).host,'taskq.host-capacity/1')))return {state:'read-only'};
    if (!w.draft) return {state: sel.length && sel.every(g => cap(g) && cap(g).edit) ? 'view' : 'read-only'};
    if (JSON.stringify(w.draft.base) !== JSON.stringify(workRevs(sel))) return {state: 'conflict'};
    return {state: w.draft.review ? 'review' : 'editing'};
  },
workStep:function(groups, w, cap, ev, newId, now) {
    const st = workStatus(groups, w, cap, now).state, l = {...w}, none = {local: w, send: []};
    const {values}=execution.module("resource-controls.workStep",{values:gs => Object.fromEntries(workFields(gs).map(f => [f.key, f.mixed ? (f.bool ? false : '') : f.values[0]]))});
    switch (ev.type) {
      case 'select': {
        if (!['view', 'read-only', 'editing'].includes(st)) return none;
        l.selected = {...w.selected, [ev.key]: !!ev.on};
        // a selection change rebinds the draft: typed values stay, Mixed is recomputed for untouched fields
        if (l.draft) { const gs = picked(groups, l); l.draft = {...l.draft, base: workRevs(gs), values: {...values(gs), ...Object.fromEntries(Object.keys(l.draft.touched).map(k => [k, l.draft.values[k]]))}, error: null}; }
        return {local: l, send: []};
      }
      case 'open':
        if (st !== 'view') return none;
        { const gs = picked(groups, w); l.draft = {base: workRevs(gs), values: values(gs), touched: {}, review: false, error: null}; }
        return {local: l, send: []};
      case 'input':
        if (!l.draft) return none;
        l.draft = {...l.draft, values: {...l.draft.values, [ev.key]: ev.value}, touched: {...l.draft.touched, [ev.key]: true}, review: false, error: null};
        return {local: l, send: []};
      case 'review': {
        if (st !== 'editing') return none;
        const errs = picked(groups, w).map(g => [g, problem(g, l.draft)]), bad = errs.find(([, e]) => e && e !== 'No change');
        l.draft = {...l.draft, error: bad ? (bad[0].labels && bad[0].labels.project || bad[0].key) + ': ' + bad[1] : errs.some(([, e]) => !e) ? null : 'No change'};
        l.draft.review = !l.draft.error;
        return {local: l, send: []};
      }
      case 'back':
        if (st === 'review') l.draft = {...l.draft, review: false};
        return {local: l, send: []};
      case 'rebase':
        if (st === 'conflict') l.draft = {...l.draft, base: workRevs(picked(groups, w)), review: false};
        return {local: l, send: []};
      case 'cancel':
        return ['editing', 'review', 'conflict'].includes(st) ? {local: freshWork(w.selected), send: []} : none;
      case 'confirm': {
        if (st !== 'review') return none;
        const send = [];
        l.locals = {};
        for (const g of picked(groups, w)) {
          const ops = changes(g, w.draft);
          if (!ops.length) continue;
          const op = {id: newId(), ops};
          l.locals[g.key] = {draft: null, op, receipt: {state: 'sending', from: revs(g)}};
          send.push({key: g.key, op});
        }
        l.draft = null;
        return {local: l, send};
      }
      case 'receipt': {
        const x = w.locals[ev.key];
        if (!x || !x.op || ev.opId !== x.op.id) return none;
        l.locals = {...w.locals, [ev.key]: {...x, receipt: {...x.receipt, state: ['saved', 'rejected', 'unknown'].includes(ev.state) ? ev.state : 'unknown', reason: ev.reason}}};
        return {local: l, send: []};
      }
      case 'retry': {
        const x = w.locals[ev.key], g = groups.find(g => g.key === ev.key);
        if (!x || !g || status(g, x, cap(g), now) !== 'unknown') return none;
        l.locals = {...w.locals, [ev.key]: {...x, receipt: {...x.receipt, state: 'sending', reason: null}}};
        return {local: l, send: [{key: ev.key, op: x.op}]};
      }
      case 'dismiss': {
        const each = workStatus(groups, w, cap, now).each || {};
        return Object.values(each).every(s => s === 'applied' || s === 'rejected') ? {local: freshWork(w.selected), send: []} : none;
      }
      default:
        return none;
    }
  },
workRender:function(groups, w, cap, now, host) {
    const ws = workStatus(groups, w, cap, now), st = ws.state, sel = picked(groups, w), d = w.draft;
    const busy = st === 'review' || st === 'receipts';
    let h = '<section class="rsc rsc-work" data-rsc-state="' + st + '" aria-label="' + esc('Work mode on ' + (host || 'this host')) + '">';
    h += '<p class="rsc-row"><span>Work mode</span><span class="rsc-muted">' + esc((host || 'this host') + ' · selected projects') + '</span></p>';
    h += '<fieldset class="rsc-scope"><legend>Projects</legend>' + groups.map(g => '<label><input type="checkbox" data-rsc-select="' + esc(g.key) + '"' + (w.selected[g.key] ? ' checked' : '') + (busy ? ' disabled' : '') + '>' + esc(g.labels && g.labels.project || g.key) + '</label>').join('') +
      '<p class="rsc-muted">Other projects remain unchanged.</p></fieldset>';
    if (sel.length) h += '<p class="rsc-muted">' + esc(sel.map(g => (g.labels && g.labels.project || g.key) + ': ' + policy(g)).join(' · ')) + '</p>';
    if (st === 'receipts') {
      for (const g of sel.filter(g => ws.each[g.key])) {
        const s = ws.each[g.key], r = w.locals[g.key].receipt;
        h += '<p class="rsc-receipt rsc-' + s + '" role="status" aria-live="polite">' + esc((g.labels && g.labels.project || g.key) + ': ' + RECEIPT[s] + (r && r.reason ? ': ' + r.reason : '')) + '</p>' +
          (s === 'unknown' ? '<button type="button" data-rsc="retry" data-rsc-key="' + esc(g.key) + '">Check again</button>' : '');
      }
      const done = Object.values(ws.each).every(s => s === 'applied' || s === 'rejected');
      return h + (done ? '<button type="button" data-rsc="dismiss">Done</button>' : '') + '</section>';
    }
    if (st === 'view' || st === 'read-only') return h + '<button type="button" data-rsc="open"' + (st === 'view' ? '' : ' disabled') + '>Edit work mode</button>' +
      (st === 'read-only' ? '<p class="rsc-muted">' + (sel.length ? 'A selected project is read-only here' : 'Select projects to change') + '</p>' : '') + '</section>';
    if (st === 'review') return h + '<div class="rsc-review" role="alert"><p>' + esc('Apply on ' + (host || 'this host') + '?') + '</p>' +
      sel.map(g => { const r = review(g, d); return '<p>' + esc((g.labels && g.labels.project || g.key) + ': ' + (r.length ? r.join(', ') : 'unchanged')) + '</p>'; }).join('') +
      '<p class="rsc-muted">Running tasks continue. Other projects, hosts and unchanged settings stay.</p></div>' +
      '<div class="rsc-actions"><button type="button" data-rsc="confirm">Confirm</button><button type="button" data-rsc="back">Back</button><button type="button" data-rsc="cancel">Cancel</button></div></section>';
    h += '<div class="rsc-form">' + workFields(sel).map(f => input('rsc-work', f, d.values[f.key], f.mixed && !d.touched[f.key], !sel.length)).join('') + '</div>';
    if (st === 'conflict') h += '<p class="rsc-alert" role="alert">' + RECEIPT.conflict + '</p>';
    if (d.error) h += '<p class="rsc-alert" role="alert">' + esc(d.error) + '</p>';
    return h + '<div class="rsc-actions">' + (st === 'conflict' ? '<button type="button" data-rsc="rebase">Review new values</button>' : '<button type="button" data-rsc="review">Review changes</button>') +
      '<button type="button" data-rsc="cancel">Cancel</button></div></section>';
  },
mountWork:function(el, adapter, selected) {
const {apply,paint}=execution.module("resource-controls.mountWork",{
apply:function(ev) {
      const r = workStep(groups, w, adapter.capability, ev, newId, now());
      w = r.local;
      if (ev.type !== 'input') paint();
      for (const x of r.send) {
        const g = groups.find(g => g.key === x.key);
        ui.effect('resource.work.send',()=>adapter.send(g,x.op),y=>apply({type:'receipt',key:x.key,opId:x.op.id,...y}));
      }
    },
paint:function() {
      const a = document.activeElement, sel = a && el.contains(a) ? (a.dataset.rscField ? '[data-rsc-field="' + a.dataset.rscField + '"]' : a.dataset.rscSelect ? '[data-rsc-select="' + CSS.escape(a.dataset.rscSelect) + '"]' : '[data-rsc="' + a.dataset.rsc + '"]') : null;
      el.innerHTML = workRender(groups, w, adapter.capability, now(), adapter.host);
      const t = sel && el.querySelector(sel);
      if (t) t.focus();
    }
});

    let groups = [], w = freshWork(selected || {});
    const {now}=execution.module("resource-controls.mountWork",{now:() => adapter.now ? adapter.now() : Math.floor(Date.now() / 1000)});
    const newId = adapter.newId || (() => (root.crypto && root.crypto.randomUUID ? root.crypto.randomUUID() : 'rsc-' + Date.now().toString(36) + Math.random().toString(36).slice(2)));


    ui.listen(el,'click', e => { const b = e.target.closest('button[data-rsc]'); if (b && !b.disabled) apply({type: b.dataset.rsc, key: b.dataset.rscKey}); });
    ui.listen(el,'change', e => { const k = e.target.dataset.rscSelect; if (k) apply({type: 'select', key: k, on: e.target.checked}); });
    ui.listen(el,'input', e => { const k = e.target.dataset.rscField; if (k) apply({type: 'input', key: k, value: e.target.type === 'checkbox' ? e.target.checked : e.target.value}); });
    return execution.module("resource-controls.mountWork.controller",{update(gs) { groups = gs; paint(); }, editing() { return !!w.draft; }});
  },
replayWork:function(fx, capability) {
    let groups = fx.groups, w = freshWork(fx.selected || {}), n = 0;
    const sent = [];
const out = [];
const {cap}=execution.module("resource-controls.replayWork.helpers",{cap:g => capability(g) || fx.capability});
    for (const ev of fx.events || []) {
      if (ev.type === 'refresh') { groups = ev.groups.map(i => fx.refresh[i]); continue; }
      if (ev.type === 'check') { const s = workStatus(groups, w, cap, fx.now); out.push({status: s.state, each: s.each || {}, html: workRender(groups, w, cap, fx.now, fx.host), sent: sent.slice()}); continue; }
      const e = ev.type === 'receipt' ? {...ev, opId: (sent.filter(x => x.key === ev.key).pop() || {op: {}}).op.id} : ev;
      const r = workStep(groups, w, cap, e, () => 'op-' + ++n, fx.now);
      w = r.local;
      sent.push(...r.send);
    }
    return out;
  },
replay:function(fx, capability) {
    let group = fx.groups[0], local = fresh(), n = 0;
    const sent = [];
const out = [];
const {cap}=execution.module("resource-controls.replay.helpers",{cap:g => capability(g) || fx.capability});
    for (const ev of fx.events || []) {
      if (ev.type === 'refresh') { group = fx.groups[ev.group]; continue; }
      if (ev.type === 'check') { out.push({status: status(group, local, cap(group), fx.now), html: render(group, local, cap(group), fx.now), sent: sent.slice(), headline: headline(group, fx.now)}); continue; }
      const r = step(group, local, cap(group), ev.type === 'receipt' ? {...ev, opId: sent.length && sent[sent.length - 1].id} : ev, () => 'op-' + ++n, fx.now);
      local = r.local;
      if (r.send) sent.push(r.send);
    }
    return out;
  }
});
// TaskQ shared resource controls (docs/design/shared-resource-controls.md). Source: https://github.com/taskqorg/taskq
// src/resource-controls.js, MIT, published with each TaskQ release as taskq-<version>-resource-controls.js and listed in
// SHA256SUMS; never edit a copy.
// Input: one node/project group of taskq.resources/1 Projection targets (host, project and runtime scopes) plus the
// adapter's labels and capability. No network client, config parser, admission rule or credentials: `send` is the adapter's.
const root=globalThis;
  const STATES = ['view', 'editing', 'review', 'conflict', 'sending', 'saved', 'applied', 'rejected', 'unknown', 'read-only','superseded'];
  const FRESH = 60;
  const {esc}=execution.module("resource-controls",{esc:s => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c]))});
  const {rtName}=execution.module("resource-controls",{rtName:rt => ({claude: 'Claude', codex: 'Codex', 'codex-app': 'Codex app'}[rt] || rt)});
  const BLOCKERS = {cpu: 'CPU', memory: 'memory', pressure: 'memory pressure', sample: 'no fresh sample', config: 'invalid config', cooldown: 'recovery',
    spacing: 'host spacing', max: 'host maximum', version: 'old service', host: 'host hold', runtime_disabled: 'runtime disabled', pause: 'paused',
    focus: 'focus', dependency: 'dependency', routing: 'routing', access: 'access', provider: 'provider limit', unknown: 'unknown reason'};
  const GATE = ['cpu', 'memory', 'pressure', 'cooldown'];

  // The group's scopes: host (no project), project (no runtime) and runtime targets, in runtime order.

  const {tkey}=execution.module("resource-controls",{tkey:t => JSON.stringify([t.target.installation_id??null,t.target.generation??null,t.target.project_id??null,t.target.runtime_id??null])});
  // Qualified Auto/Fixed authority versus the legacy Fixed guard; a consumer never infers it from a version string.
  const {schemaOf}=execution.module("resource-controls",{schemaOf:(x, s) => x && (x.supported || []).find(p => p.policy_schema === s)});
  // The schema that advertises `field` at this target's scope; null: not editable here.
  const {schemaFor}=execution.module("resource-controls",{schemaFor:(x, field) => { const p = x && (x.supported || []).find(p => p.fields.includes(field) && p.scopes.includes(x.target.runtime_id != null ? 'runtime' : x.target.project_id != null ? 'project' : 'host')); return p ? p.policy_schema : null; }});
  const {editable}=execution.module("resource-controls",{editable:(x, field) => !!schemaFor(x, field)});

  // Seconds since the producer's sample (unknown: null) and whether the observation itself is fresh.


  // Policy line: Auto/Fixed only under the qualified admission schema, else the legacy guard.


  // Load gate text of taskq.md § 4 from the producer's blockers; unknown is never Off or Ready.




  // used/limit per runtime: unknown stays `?`, explicit zero is off, absent Fixed is none, Auto absent has no cap.


  // Saved versus applied for every target: applied needs a fresh observation whose effective revision equals the saved one.




  // Editor fields of the group in the parent's order (menu M12): host mode and maximum, runtime limits, then the
  // project load gate. Mode and maximum appear only where the producer advertises taskq.admission/1.

  const MODES = {auto: 'Auto', fixed: 'Fixed'};
  const {shown}=execution.module("resource-controls",{shown:f => f.bool ? !!f.value : f.value == null ? '' : String(f.value)});
  const {word}=execution.module("resource-controls",{word:(f, v) => f.bool ? (v ? 'on' : 'off') : f.choice ? MODES[v] || 'unknown' : v === '' ? f.none || 'none' : v});
  const {said}=execution.module("resource-controls",{said:f => word(f, shown(f))});

  // Explicitly edited fields that differ from the saved policy; untouched fields never travel.




  // One operation per target, contract-shaped (taskq.resources/1 Changes); load_gate travels whole.


  // Review lines: exact old → new per edited field.


  // Local state: an editor draft bound to the revisions it was opened against, one operation and its receipt.

  const {revs}=execution.module("resource-controls",{revs:group => Object.fromEntries((group.targets || []).map(t => [tkey(t), t.current_revision]))});



  // Pure transition: (local, event) -> {local, send}. Events: open, input {key, value}, review, back, cancel, confirm,
  // rebase, receipt {opId, state, reason}, retry, dismiss.


  const RECEIPT = {sending: 'Saving…', saved: 'Saved; waiting for service', applied: 'Applied: read back from the service',superseded:'Saved; replaced by a newer command',
    rejected: 'Not saved', unknown: 'Delivery unknown: check again', conflict: 'Settings changed since you opened the editor'};



  // One editor field: label then control; Mixed (Work mode) shows no common value until the field is touched.


  // Browser binding: one element per group. adapter = {capability(group) -> {edit, reason, note}, send(group, op) ->
  // Promise<{state, reason}>, now() -> seconds, newId?()}. update(group) keeps the draft, focus and caret.


  // Work mode (menu M12): one draft over explicitly selected project groups of one host. A field whose selected
  // values differ shows Mixed; only touched fields travel, each group keeps its own untouched values and gets its own
  // operation and receipt (partial failure stays per project). Unselected groups are never sent.
  const {RANK}=execution.module("resource-controls",{RANK:k => k.startsWith('host.') ? 0 : k.startsWith('rt:') ? 1 : 2});

  const {picked}=execution.module("resource-controls",{picked:(groups, w) => groups.filter(g => w.selected[g.key])});
  const {workRevs}=execution.module("resource-controls",{workRevs:gs => Object.fromEntries(gs.map(g => [g.key, revs(g)]))});


  // Per selected group: its Capacity status (sending, saved, applied, rejected, unknown) once an operation exists.


  // Pure transition like `step`. Events: select {key, on}, open, input, review, back, rebase, cancel, confirm,
  // receipt {key, opId, state, reason}, retry {key}, dismiss. send: [{key, op}].




  // Browser binding of Work mode: adapter as in `mount`, plus `host`. update(groups) keeps selection, draft and focus.


  // Work mode conformance replay: case `{groups, now, capability, selected, events}`; refresh {groups: [i..]} replaces
  // the observed groups; check returns {status, each, html, sent}.


  // Conformance replay shared by every consumer harness: the fixture's events through `step`; each check returns
  // {status, html, sent, headline}. Refresh {group} replaces the observed group; now comes from the case.


  const api = {STATES, scopes, freshness, policy, gate, usage, applied, headline, fields, changes, review, status, step, render, mount, replay,
    workFields, workStatus, workStep, workRender, mountWork, replayWork};
  module.exports=api;
