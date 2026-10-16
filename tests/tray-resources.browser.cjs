// Chain: real tray HTML -> shared resource controls -> `capacity` IPC. Instrument with Playwright; not native tray proof.
const fs=require('node:fs'),assert=require('node:assert/strict'),{chromium}=require('playwright');
const html=fs.readFileSync('build/web/tray.html','utf8');
const fx=JSON.parse(fs.readFileSync('tests/fixtures/resource-controls.json','utf8'));
const conflict=fx.cases.find(c=>/conflict/.test(c.name)).groups,over=fx.cases.find(c=>/over-capacity/.test(c.name)).groups[0];
(async()=>{
  const browser=await chromium.launch(require('./browser.cjs')());
  try{
    for(const theme of ['light','dark'])for(const font of [16,20])for(const width of [280,font===20?360:320]){
      const page=await browser.newPage({viewport:{width,height:900},colorScheme:theme});
      const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await page.evaluate(()=>{window.messages=[];window.ipc={postMessage:s=>window.messages.push(JSON.parse(s))};});
      await page.setContent(html);
      const root='p',saved={limits:{claude:2},load_gate:{enabled:true,cpu_max:85,memory_min:10}};
      const state=r=>({page:root,font,theme,now:1000,projects:[{root,name:'taskq',issues:'',alive:true,age:10,complete:true,focus:[],labels:[],tasks:[],cap_host:'mac',cap_saved:saved,resources:r}],ai:{draft:''},notify:{projects:{}}});
      await page.evaluate(s=>taskq.receive(s),state(over));
      await page.locator('summary',{hasText:'Capacity'}).click();
      assert.match(await page.locator('.rsc').innerText(),/3\/1 · over limit/);
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'over-capacity row fits');
      await page.evaluate(s=>taskq.receive(s),state(conflict[0]));
      await page.locator('[data-rsc=open]').click();
      const claude=page.locator('[data-rsc-field="rt:claude"]');
      await claude.fill('3');await claude.focus();
      await page.evaluate(s=>taskq.receive(s),state(conflict[0]));
      assert.deepEqual(await claude.evaluate(t=>({v:t.value,focused:document.activeElement===t})),{v:'3',focused:true},'tick keeps draft and focus');
      await page.evaluate(s=>taskq.receive(s),state(conflict[1]));
      await page.locator('[data-rsc=cancel]').focus();
      await page.waitForFunction(()=>document.querySelector('.rsc').dataset.rscState==='conflict');
      assert.equal(await claude.inputValue(),'3');
      await page.locator('[data-rsc=rebase]').click();await page.locator('[data-rsc=review]').click();
      assert.match(await page.locator('.rsc-review').innerText(),/Claude 4 → 3/);
      await page.locator('[data-rsc=confirm]').click();
      await page.waitForFunction(()=>messages.some(m=>m.a==='capacity'));
      const m=await page.evaluate(()=>messages.find(m=>m.a==='capacity'));
      assert.deepEqual(m.set,{limits:{claude:3}});assert.equal(m.root,root);
      await page.evaluate(x=>taskq.receive({capmsg:x}),{root,id:m.id,text:'Saved: claude 4 → 3'});
      await page.waitForFunction(()=>document.querySelector('.rsc').dataset.rscState==='saved');
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
      assert.deepEqual(errors,[]);
      console.log('PASS tray resource controls over-capacity, draft/focus on tick, conflict rebase, review, capacity IPC, saved;',theme,font+'px',width+'px');
      await page.close();
    }
  }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exit(1);});
