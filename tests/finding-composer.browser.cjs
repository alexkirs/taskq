// Development instrument: generate a self-checking browser page, then open it in a browser.
// Usage: node tests/finding-composer.browser.cjs /tmp/taskq-949.html
// No live board/native-adapter qualification. Screenshots and owner visual acceptance stay separate.
const fs=require('node:fs');
const fx=require('./fixtures/finding-composer.json'),answer=require('./fixtures/answer-card.json').cases[0];
const css=['project-blocks','answer-card','finding-composer'].map(n=>fs.readFileSync('src/'+n+'.css','utf8')).join('\n');
const scripts=['project-blocks','answer-card','finding-composer'].map(n=>fs.readFileSync('src/'+n+'.js','utf8')).join('\n');
async function check(fx,answer){
  const eq=(a,b,why)=>{if(JSON.stringify(a)!==JSON.stringify(b))throw Error(why+': '+JSON.stringify(a)+' != '+JSON.stringify(b));};
  const assert=(yes,why)=>{if(!yes)throw Error(why);};
  const el=document.querySelector('#app'),find=s=>el.querySelector(s),tick=()=>new Promise(r=>setTimeout(r,0));
  let saved={},sent=[],checked=[],nav=[],allowed,result,composer,failCheck=false;
  const mount=()=>TaskqFindingComposer.mount(el,{
    capability:()=>allowed,send:async r=>{sent.push(r);return result;},reconcile:async r=>{checked.push(r);if(failCheck)throw Error('Read failed');return result;},
    saveDraft:(source,d)=>saved[source]=d,loadDraft:source=>saved[source],navigation:{project:id=>nav.push(['project',id]),tasks:(id,s)=>nav.push(['tasks',id,s]),open:u=>nav.push(['open',u])},
    answer:{capability:()=>({answer:true}),send:async()=>({state:'accepted'})}
  });
  const setup=()=>{saved={};sent=[];checked=[];nav=[];allowed={create:true,start:true};result=fx.receipts.held;failCheck=false;composer=mount();composer.update(fx);};
  const input=(field,value)=>{const e=find('[data-field="'+field+'"]');e.focus();if(e.type==='checkbox')e.checked=value;else e.value=value;e.dispatchEvent(new Event('input',{bubbles:true}));};
  const click=s=>find(s).click();
  const review=()=>input('reviewed',true);
  const text=()=>find('[data-finding-form]').textContent;
  setup();eq(find('[data-field=start]').value,'no','held default');assert(find('[data-finding=send]').disabled,'review required');
  input('goal','Edited scope; do not restart services.');composer.update(fx);eq(find('[data-field=goal]').value,'Edited scope; do not restart services.','refresh draft');
  assert(find('[data-field=goal]')===document.activeElement,'refresh focus');
  click('[data-tqp=tasks][data-state=review]');click('[data-tqp=project]');click('.tqp a');
  eq(nav,[['tasks','serverq','review'],['project','serverq'],['open',fx.projects[0].board.url]],'independent routes');
  const radio=find('[data-field=project][value=taskq]');radio.checked=true;radio.dispatchEvent(new Event('input',{bubbles:true}));review();
  assert(document.documentElement.scrollWidth<=innerWidth,'narrow overflow');
  const existing=find('.tqp-project'),control=document.createElement('div');control.style.width=existing.getBoundingClientRect().width+'px';
  control.innerHTML=TaskqProjectBlocks.summary(fx.projects[0]);document.body.append(control);
  const look=root=>[...root.querySelectorAll('*')].map(el=>{const c=getComputedStyle(el);return ['fontSize','lineHeight','color','backgroundColor','padding','gap','borderRadius','justifyContent'].map(k=>c[k]);});
  eq(look(existing),look(control.firstElementChild),'embedded block equals standalone pinned parent');control.remove();
  click('[data-finding=send]');await tick();eq(sent.length,1,'one create');eq(sent[0].board,fx.projects[1].board.url,'override board');eq(sent[0].start,false,'held request');
  assert(text().includes('Task #2401 recorded'),'create receipt');assert(!find('[data-finding=send]'),'immutable send');click('[data-tqp=tasks][data-state=review]');
  const linked={project:fx.projects[0],row:answer.rows[0]};composer.update({...fx,linked});
  const area=find('.qac textarea');area.closest('details').open=true;area.focus();area.value='Preserve my answer';area.dispatchEvent(new Event('input',{bubbles:true}));
  composer.update({...fx,linked});eq(find('.qac textarea').value,'Preserve my answer','answer draft');assert(find('.qac textarea')===document.activeElement,'answer focus');
  assert(find('[data-finding-form]').hidden,'existing task prevents duplicate');assert(find('.tqp-task'),'canonical linked row');
  setup();input('title','Preserved title');click('[data-finding=cancel]');click('[data-finding=resume]');eq(find('[data-field=title]').value,'Preserved title','cancel draft');
  allowed={create:false,reason:'Missing create grant'};composer.update(fx);assert(text().includes('Missing create grant'),'capability reason');
  allowed={create:true,start:true};const changed={...fx,finding:{...fx.finding,revision:2,sanitizedEvidence:'New observation: 8%'}};composer.update(changed);
  assert(text().includes('Source changed')&&text().includes('New observation: 8%'),'stale source review');click('[data-finding=review-source]');review();
  result=fx.receipts.denied;click('[data-finding=send]');await tick();assert(text().includes('Write access denied'),'denied receipt');eq(find('[data-field=title]').value,'Preserved title','denied draft');
  setup();result=fx.receipts.unknown;review();const send=find('[data-finding=send]');send.click();send.click();await tick();eq(sent.length,1,'double submit');
  composer=mount();composer.update(fx);result=fx.receipts.held;click('[data-finding=reconcile]');await tick();eq(checked[0],sent[0],'reload immutable reconcile');eq(sent.length,1,'reconcile never creates');
  setup();input('start','yes');review();result=fx.receipts.partial;click('[data-finding=send]');await tick();eq(sent[0].start,true,'explicit start');
  failCheck=true;click('[data-finding=reconcile]');await tick();assert(text().includes('Read failed')&&text().includes('Task #2401 recorded'),'partial receipt retained on failed read');
  setup();saved={};composer=mount();composer.update({...fx,finding:{...fx.finding,mapping:undefined}});assert(!find('[data-field=project]:checked'),'no inferred project');
  // Leave the exact long-content fixture for manual inspection, not a synthetic pass screen.
  setup();assert(document.documentElement.scrollWidth<=innerWidth,'final fixture overflow');
}
const frames=[];for(const theme of fx.themes)for(const font of fx.fonts)for(const width of fx.widths){
  const label=`${theme} ${font}px ${width}px`;
  const script=scripts+'\n('+check.toString()+')('+JSON.stringify(fx)+','+JSON.stringify(answer)+').then(()=>parent.postMessage({label:'+JSON.stringify(label)+',ok:true},"*")).catch(e=>parent.postMessage({label:'+JSON.stringify(label)+',error:e.stack},"*"));';
  const html='<!doctype html><style>:root{color-scheme:'+theme+'}body{margin:8px;font:'+font+'px/1.4 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;background:light-dark(#fafbfc,#252b32)}'+css+'</style><div id="app"></div><script>'+script+'</script>';
  frames.push('<section><h2>'+label+'</h2><iframe title="'+label+'" style="width:'+width+'px;height:1600px;border:1px solid #888" srcdoc="'+html.replace(/&/g,'&amp;').replace(/"/g,'&quot;')+'"></iframe></section>');
}
const out='<!doctype html><title>Finding composer checks</title><style>body{font:16px sans-serif}main{display:flex;gap:16px;flex-wrap:wrap}h2{font:inherit}pre{white-space:pre-wrap}</style><h1>Finding composer browser checks</h1><pre id="results">Running 8 fixtures…</pre><main>'+frames.join('')+'</main><script>const results=[];window.addEventListener("message",e=>{if(!e.data.label)return;results.push(e.data);document.querySelector("#results").textContent=results.map(r=>(r.ok?"PASS ":"FAIL ")+r.label+(r.error?"\\n"+r.error:"")).join("\\n");document.title=results.length===8?results.every(r=>r.ok)?"PASS 8 finding composer fixtures":"FAIL finding composer fixtures":"Running finding composer checks";});</script>';
if(!process.argv[2])throw Error('Supply an output HTML path');fs.writeFileSync(process.argv[2],out);console.log('Wrote '+process.argv[2]+'; open in a browser and require PASS for all 8 fixtures.');
