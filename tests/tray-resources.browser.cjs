// Chain: bundled tray -> qualified host editor -> typed IPC -> exact observed revision. Browser, not installed proof.
const fs=require('node:fs'),assert=require('node:assert/strict'),path=require('node:path'),{chromium}=require('playwright');
const html=fs.readFileSync('build/web/tray.html','utf8');
const fx=JSON.parse(fs.readFileSync('tests/fixtures/resource-controls.json','utf8'));
const modern=fx.cases.find(c=>c.name.startsWith('host commands'));
(async()=>{
  const browser=await chromium.launch(require('./browser.cjs')());
  try{
    for(const theme of ['light','dark'])for(const font of [16,20])for(const width of [280,font===20?360:320]){
      const page=await browser.newPage({viewport:{width,height:900},colorScheme:theme});
      const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await page.evaluate(()=>{window.messages=[];window.ipc={postMessage:s=>window.messages.push(JSON.parse(s))};});
      await page.setContent(html);
      const root='p',saved={limits:{claude:2,codex:3},load_gate:{enabled:true,cpu_max:90,memory_min:10}};
      const state=r=>({page:root,font,theme,now:1000,service:{state:'running',pid:731,fresh:false},projects:[{root,name:'taskq',issues:'',alive:true,age:10,complete:true,focus:[],labels:[],tasks:[],cap_host:'mac',cap_saved:saved,resources:r}],ai:{draft:''},notify:{projects:{}}});
      const fits=async()=>assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'host editor fits '+theme+'/'+font+'/'+width);
      const shot=async name=>{if(process.env.TASKQ_TEST_ARTIFACTS&&font===20){fs.mkdirSync(process.env.TASKQ_TEST_ARTIFACTS,{recursive:true});await page.evaluate(()=>scrollTo(0,0));await page.screenshot({path:path.join(process.env.TASKQ_TEST_ARTIFACTS,'host-capacity-'+theme+'-'+name+'-'+font+'-'+width+'.png'),fullPage:true});}};
      const over=structuredClone(modern.groups[0]);over.targets[0].runtime_usage=[{runtime_id:'codex',used:3,limit:1,limit_kind:'ceiling'}];
      await page.evaluate(s=>taskq.receive(s),state(over));
      await page.locator('summary',{hasText:'Capacity'}).click();
      const service=page.locator('details.service');
      if(!await service.evaluate(e=>e.open))await service.locator('summary').first().click();
      assert.match(await service.innerText(),/Host service: running · PID 731 · stale observation/);
      assert.match(await page.locator('.rsc').innerText(),/3\/1 · over limit/);await fits();
      await page.evaluate(s=>taskq.receive(s),state(modern.groups[0]));
      await page.locator('[data-rsc=open]').click();
      const codex=page.locator('[data-rsc-field="rt:codex"]');
      await codex.fill('4');await codex.focus();
      await page.evaluate(s=>taskq.receive(s),state(modern.groups[0]));
      assert.deepEqual(await codex.evaluate(t=>({v:t.value,focused:document.activeElement===t})),{v:'4',focused:true},'tick keeps draft and focus');
      assert.equal(await page.locator('[data-rsc-field="gate.enabled"]').isDisabled(),false,'host gate is independently editable');
      assert.equal(await page.locator('[data-rsc-field="host.mode"]').count(),0,'no Fixed/Auto selector');
      await fits();await shot('editor');
      await page.evaluate(s=>taskq.receive(s),state(modern.groups[4]));
      await page.locator('[data-rsc=cancel]').focus();
      await page.waitForFunction(()=>document.querySelector('.rsc').dataset.rscState==='conflict');
      assert.equal(await codex.inputValue(),'4');
      await page.locator('[data-rsc=rebase]').click();await page.locator('[data-rsc=review]').click();
      assert.match(await page.locator('.rsc-review').innerText(),/Codex 5 → 4/);
      assert.match(await page.locator('.rsc-review').innerText(),/all registered projects/);
      await fits();await shot('review');
      await page.locator('[data-rsc=confirm]').click();
      await page.waitForFunction(()=>messages.some(m=>m.a==='capacity'));
      const m=await page.evaluate(()=>messages.find(m=>m.a==='capacity'));
      assert.equal(m.root,root);assert.equal(m.issued_at,1000);assert.equal(m.expires_at,1300);
      assert.equal(m.ops.length,1);assert.equal(m.ops[0].target.project_id,null);
      assert.equal(m.ops[0].policy_schema,'taskq.host-capacity/1');assert.deepEqual(m.ops[0].changes,{runtime_limit:4});
      assert.equal(m.ops[0].expected_revision,'3'.repeat(32));assert.equal(m.set,undefined,'legacy converter is retired');
      await page.evaluate(x=>taskq.receive({capmsg:x}),{root,id:m.id,state:'saved',saved_revision:'4'.repeat(32)});
      await page.waitForFunction(()=>document.querySelector('.rsc').dataset.rscState==='saved');
      const pending=structuredClone(modern.groups[3]);
      for(const t of pending.targets){t.current_revision='4'.repeat(32);t.effective_revision='5'.repeat(32);}
      await page.evaluate(s=>taskq.receive(s),state(pending));
      assert.equal(await page.locator('.rsc').getAttribute('data-rsc-state'),'saved','equal values with a different receipt cannot confirm application');
      for(const t of pending.targets)t.effective_revision=t.current_revision;
      await page.evaluate(s=>taskq.receive(s),state(pending));
      await page.waitForFunction(()=>document.querySelector('.rsc').dataset.rscState==='applied');await fits();
      await page.locator('[data-a=work]').click();
      assert.equal(await page.locator('[data-rsc-select]').count(),0,'project checkboxes cannot narrow a host edit');
      assert.match(await page.locator('.rsc').getAttribute('aria-label'),/all registered projects/);await fits();
      await page.locator('[data-a=back]').click();
      await page.evaluate(s=>taskq.receive(s),state(fx.cases[0].groups[0]));
      const summary=page.locator('summary',{hasText:'Capacity'});
      if(!await page.locator('details').filter({has:summary}).evaluate(e=>e.open))await summary.click();
      assert.equal(await page.locator('.rsc button').count(),0,'legacy source retires the editor and stale receipt');
      assert.deepEqual(errors,[]);
      console.log('PASS host scope, bounded typed IPC, draft/focus, conflict, exact acknowledgment and legacy refusal;',theme,font+'px',width+'px');
      await page.close();
    }
  }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exit(1);});
