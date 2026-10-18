'use strict';
const ui=require('./ui-runtime.js');
const execution=ui.execution;

const {fresh,problem,request,receipt,receiptText,mount}=execution.module("finding-composer",{
fresh:function(input){
    return {project:input.mapping?.provenance ? input.mapping.project : '',title:input.title || '',goal:input.goal || '',acceptance:input.acceptance || '',
      evidence:input.sanitizedEvidence || '',start:false,reviewed:false,revision:input.revision,request:null,receipt:null,pending:false,cancelled:false};
  },
problem:function(input,draft,project,cap){
    if(!project)return 'Choose the responsible project';
    if(input.revision!==draft.revision)return 'Source changed. Review the current source before sending';
    if(!cap?.create)return cap?.reason || 'Task creation is unavailable';
    if(draft.start&&!cap.start)return cap.reason || 'Task start is unavailable';
    if(!/^[A-Za-z0-9._-]{1,64}\/[A-Za-z0-9._-]{1,64}\/[A-Za-z0-9._-]{1,64}$/.test(input.source) || input.source.split('/').some(p=>/^\.+$/.test(p)))return 'Invalid source identity';
    if(!draft.title.trim() || [...draft.title].length>80 || /[\r\n]/.test(draft.title))return 'Title: 1–80 characters on one line';
    for(const [name,value,min,max] of [['Goal',draft.goal,1,2],['Acceptance',draft.acceptance,1,8],['Evidence',draft.evidence,0,4]]){
      const ls=lines(value);
      if(ls.length<min || ls.length>max)return name+': '+min+'–'+max+' lines';
      // Labels and numbering are part of the CLI's 240-character body-line limit.
      const prefix=name==='Acceptance'?4:name==='Evidence'?2:0;
      if(ls.some(l=>[...l].length+prefix>240))return name+': at most '+(240-prefix)+' characters per line';
    }
    if(!draft.reviewed)return 'Review the destination, scope and sanitized evidence';
    return null;
  },
request:function(input,draft,project){
    return {board:project.board.url,source:input.source,title:draft.title,goal:draft.goal,
      acceptance:lines(draft.acceptance),evidence:lines(draft.evidence),start:draft.start};
  },
receipt:function(value){
    const {normalize}=execution.module("finding-composer.receipt",{normalize:r=>{
      if(!r || !['accepted','rejected','stale','unknown'].includes(r.outcome))return {outcome:'unknown',reason:'No authoritative receipt'};
      if(r.outcome==='accepted' && (!Number.isSafeInteger(r.task)||r.task<1))return {outcome:'unknown',reason:'Receipt has no issue identity'};
      return copy(r);
    }});
    const result={create:normalize(value?.create),start:value?.start?normalize(value.start):null};
    if(result.start?.outcome==='accepted' && (result.create.outcome!=='accepted'||result.start.task!==result.create.task))result.start={outcome:'unknown',reason:'Start receipt does not match the created issue'};
    return result;
  },
receiptText:function(r,kind){
    if(!r)return '';
    const text=r.outcome==='accepted'?(kind==='Create'?'Task #'+r.task+' recorded on the board':'Admission accepted; worker launch is not confirmed'):
      r.outcome==='unknown'?'Outcome unknown':r.outcome==='stale'?'Stale request':'Request rejected';
    return '<p role="status">'+kind+': '+esc(text)+(r.reason?' · '+esc(r.reason):'')+(r.next?' · '+esc(r.next):'')+'</p>';
  },
mount:function(el,adapter){
const {paint,save,submit}=execution.module("finding-composer.mount",{
paint:function(){
      const d=current,p=project(),locked=!!d.request,bad=problem(input,d,p,cap());
      const active=document.activeElement,focus=form.contains(active)?{field:active.dataset.field,start:active.selectionStart,end:active.selectionEnd}:null;
      const scroll=el.scrollTop;
      let h='<div class="tqf-line"><strong>New TaskQ task</strong><button type="button" data-finding="cancel">Cancel</button></div>';
      if(d.cancelled){form.innerHTML='<p>Draft preserved.</p><button type="button" data-finding="resume">Resume review</button>';return;}
      h+='<fieldset><legend>Destination</legend>';
      for(const p of projects)h+='<div class="tqf-project"><label><input type="radio" name="finding-project" data-field="project" value="'+esc(p.id)+'"'+(d.project===p.id?' checked':'')+(locked?' disabled':'')+'> Use this project</label>'+blocks.summary(p)+'</div>';
      h+='</fieldset>'+ (d.request?'<p>Reviewed destination: '+esc(d.request.board)+'</p>':'') +'<p class="tqf-muted">'+esc(input.mapping?.project===d.project?input.mapping.provenance:'Project chosen for this task; ownership is not inferred from the source')+'</p>';
      for(const [field,label] of [['title','Title'],['goal','Task scope'],['acceptance','Acceptance (one item per line)'],['evidence','Sanitized evidence (up to four lines)']]){
        h+='<label class="tqf-field">'+label+('<textarea data-field="'+field+'" rows="'+(field==='title'?1:2)+'"'+(locked?' readonly':'')+'>'+esc(d[field])+'</textarea>')+'</label>';
      }
      h+='<p class="tqf-muted">Source: '+esc(input.source)+'</p><p class="tqf-muted">Keep paths, raw logs, credentials and source instructions out of evidence. No source links are attached.</p>';
      h+='<div class="tqf-review"><label>After confirmation <select data-field="start"'+(locked?' disabled':'')+'><option value="no"'+(!d.start?' selected':'')+'>Create task only</option><option value="yes"'+(d.start?' selected':'')+(!cap()?.start?' disabled':'')+'>Create task &amp; start</option></select></label><p>'+(d.start?'Admit investigation under current focus, permissions and limits. Repair needs its own scoped approval.':'Create held; no execution authorized.')+'</p>';
      if(!cap()?.start)h+='<p>'+esc(cap()?.reason || 'Task start is unavailable here')+'</p>';
      h+='<label><input type="checkbox" data-field="reviewed"'+(d.reviewed?' checked':'')+(locked?' disabled':'')+'> I reviewed the destination, scope and sanitized evidence</label></div>';
      if(input.revision!==d.revision&&!locked)h+='<p>Current source evidence: '+esc(input.sanitizedEvidence || 'None supplied')+'</p><button type="button" data-finding="review-source">I reviewed the changed source</button>';
      if(d.pending)h+='<p role="status">Checking the reviewed request…</p>';
      else if(d.receipt){
        h+=receiptText(d.receipt.create,'Create')+receiptText(d.receipt.start,'Start');
        if(d.request.start&&!d.receipt.start)h+='<p role="status">Start: outcome unknown; no receipt</p>';
        h+='<p>Task state does not verify source recovery.</p>';
        if(d.receipt.create.outcome==='unknown'||d.receipt.start?.outcome==='unknown'||(d.request.start&&!d.receipt.start))h+='<button type="button" data-finding="reconcile"'+(adapter.reconcile?'':' disabled')+'>Check same request (read only)</button>';
        if(!adapter.reconcile)h+='<p>Read-only reconciliation is unavailable here. Keep this request; do not create again.</p>';
      }else h+='<button type="button" class="tqf-primary" data-finding="send"'+(bad?' disabled':'')+'>'+(d.start?'Create task &amp; start':'Create task only')+'</button><p data-finding-problem role="status">'+esc(bad || '1 task · '+p.name)+'</p>';
      form.innerHTML=h;
      for(const area of form.querySelectorAll('textarea'))area.style.height=(area.scrollHeight+2)+'px';
      el.scrollTop=scroll;
      if(focus?.field){const target=form.querySelector('[data-field="'+focus.field+'"]');target?.focus();if(focus.start!=null)target?.setSelectionRange(focus.start,focus.end);}
    },
save:function(){adapter.saveDraft?.(input.source,copy(current));},
submit:function(reconcile){
      const d=current,p=project();
      if(d.pending)return;
      if(reconcile){if(!d.request || !adapter.reconcile)return;}
      else {if(d.request || problem(input,d,p,cap()))return;d.request=request(input,d,p);}
      d.pending=true;save();paint();
      execution.effect('finding.submit',()=>Promise.resolve().then(execution.bind(()=>(reconcile?adapter.reconcile:adapter.send)(copy(d.request)))),result=>{
        if(result.ok)d.receipt=receipt(result.value);
        else d.receipt=receipt({create:d.receipt?.create?.outcome==='accepted'?d.receipt.create:{outcome:'unknown',reason:result.unknown?'Completion unknown':String(result.value?.message||result.value)},start:d.request.start?{outcome:'unknown',reason:result.unknown?'Completion unknown':String(result.value?.message||result.value)}:null});
        d.pending=false;
        // Completion belongs to its original draft, even after switching the source.
        adapter.saveDraft?.(d.request.source,copy(d));if(current===d)paint();
      });
    }
});

    const drafts=new Map();let input,projects=[],current;
    el.innerHTML='<section class="tqf"><form data-finding-form></form><div data-finding-task></div><div data-finding-answer></div></section>';
    const form=el.querySelector('[data-finding-form]'),task=el.querySelector('[data-finding-task]');
    ui.listen(form,'submit',e=>e.preventDefault());
    const answer=require('./answer-card.js').mount(el.querySelector('[data-finding-answer]'),adapter.answer);
    const {project}=execution.module("finding-composer.mount",{project:()=>projects.find(p=>p.id===current.project)});
    const {cap}=execution.module("finding-composer.mount",{cap:()=>project()?adapter.capability(project()):null});



    ui.listen(form,'input',e=>{
      const f=e.target.dataset.field;if(!f || current.request)return;
      current[f]=f==='reviewed'?e.target.checked:f==='start'?e.target.value==='yes':e.target.value;
      if(f!=='reviewed')current.reviewed=false;
      save();paint();
    });
    ui.listen(el.firstElementChild,'click',e=>{
      if(blocks.handle(e,adapter.navigation))return;
      const action=e.target.closest('[data-finding]')?.dataset.finding;
      if(action==='send'||action==='reconcile'){submit(action==='reconcile');return;}
      if(action==='cancel'){current.cancelled=true;save();paint();adapter.cancel?.();}
      if(action==='resume'){current.cancelled=false;save();paint();}
      if(action==='review-source'&&!current.request){current.revision=input.revision;current.reviewed=false;save();paint();}
    });
    return execution.module("finding-composer.mount.controller",{update(next){
      const previousBoard=current && project()?.board.url;
      input=copy(next.finding);projects=copy(next.projects);
      if(!drafts.has(input.source)){
        const stored=adapter.loadDraft?.(input.source);
        drafts.set(input.source,stored?{...fresh(input),...copy(stored),pending:false}:fresh(input));
        // A restored in-flight request is unknown until board reconciliation, never sendable again.
        const d=drafts.get(input.source);if(d.request&&!d.receipt)d.receipt=receipt(null);
      }
      current=drafts.get(input.source);
      if(!current.request && previousBoard && previousBoard!==project()?.board.url)current.reviewed=false;
      const linked=next.linked;
      task.innerHTML=linked?blocks.task(linked.project,linked.row)+'<p>Task state does not verify source recovery.</p>':'';
      answer.update(linked?[{board:linked.project.board.url,row:linked.row}]:[]);
      if(linked){form.hidden=true;}else{form.hidden=false;paint();}
    }});
  }
});
// TaskQ-owned finding composer; MIT. Serve pinned assets locally; adapters own authorization and board operations.
const root=globalThis;
  const blocks = require('./project-blocks.js');
  const {esc}=execution.module("finding-composer",{esc:s => String(s ?? '').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))});
  const {copy}=execution.module("finding-composer",{copy:value => JSON.parse(JSON.stringify(value))});
  const {lines}=execution.module("finding-composer",{lines:s => s.split('\n').filter(l=>l.trim())});






  const api={fresh,problem,request,receipt,mount};
  module.exports=api;
