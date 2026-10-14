// Chain: real tray HTML -> shared card -> existing IPC. Instrument with Playwright; not native tray/board proof.
const fs=require('node:fs'),assert=require('node:assert/strict'),{chromium}=require('playwright');
const html=fs.readFileSync('src/tray.html','utf8')
  .replace('{{ANSWER_CARD_CSS}}',fs.readFileSync('src/answer-card.css','utf8'))
  .replace('{{ANSWER_CARD_JS}}',fs.readFileSync('src/answer-card.js','utf8'))
  .replace('{{PROJECT_BLOCKS_CSS}}',fs.readFileSync('src/project-blocks.css','utf8'))
  .replace('{{PROJECT_BLOCKS_JS}}',fs.readFileSync('src/project-blocks.js','utf8'))
  .replace('{{RESOURCE_CONTROLS_CSS}}',fs.readFileSync('src/resource-controls.css','utf8'))
  .replace('{{RESOURCE_CONTROLS_JS}}',fs.readFileSync('src/resource-controls.js','utf8'));
const fx=JSON.parse(fs.readFileSync('tests/fixtures/answer-card.json','utf8'));
(async()=>{
  const browser=await chromium.launch(require('./browser.cjs')());
  try{
    for(const theme of ['light','dark'])for(const font of [16,20]){
      const page=await browser.newPage({viewport:{width:font===20?360:320,height:900},colorScheme:theme});
      const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await page.setContent(html);
      await page.evaluate(()=>{window.messages=[];window.ipc={postMessage:s=>window.messages.push(JSON.parse(s))};});
      const board='https://github.com/o/r/issues',root='project',rows=fx.cases.find(c=>c.name.startsWith('stale')).rows;
      const state={page:root,font,theme,projects:[{root,name:'project',issues:board,alive:true,complete:true,focus:[],labels:[],tasks:[rows[0]],cap_saved:{error:'not configured'}}],ai:{draft:''},notify:{projects:{}}};
      await page.evaluate(s=>taskq.receive(s),state);
      await page.locator('.qac-other summary').click();const text=page.locator('.qac textarea');await text.fill('keep draft');await text.focus();
      await text.evaluate(t=>t.setSelectionRange(2,4));
      await page.evaluate(s=>taskq.receive(s),state);
      assert.deepEqual(await text.evaluate(t=>({draft:t.value,focused:document.activeElement===t,caret:[t.selectionStart,t.selectionEnd]})),{draft:'keep draft',focused:true,caret:[2,4]});
      state.projects[0].tasks=[rows[1]];await page.evaluate(s=>taskq.receive(s),state);
      assert.equal(await page.locator('.qac').getAttribute('data-qac-state'),'stale');
      assert.equal(await text.inputValue(),'keep draft');
      assert(await page.locator('.qac button[type=submit]').isDisabled());
      await page.locator('[data-qac=review]').click();await text.fill('2');
      await page.locator('.qac button[type=submit]').click();
      await page.waitForFunction(()=>messages.some(m=>m.a==='answer'));
      const op=await page.evaluate(()=>messages.find(m=>m.a==='answer').op);
      assert.equal(op.text,'2');assert.equal(op.choice,undefined);assert.equal(op.revision,rows[1].revision);
      assert.equal(await page.locator('.qac').getAttribute('data-qac-state'),'sending');
      await page.evaluate(s=>taskq.receive(s),state);
      assert.equal(await text.inputValue(),'2');
      // A1: no question card remains outside ask, even while its receipt is pending.
      state.projects[0].tasks=[{...rows[1],state:'doing',revision:rows[1].revision+1}];
      await page.evaluate(s=>taskq.receive(s),state);
      assert.equal(await page.locator('.qac').count(),0);
      assert((await page.locator('[role=status]').textContent()).includes('sending'));
      await page.evaluate(r=>taskq.receive({answerReceipt:r}),{root,id:op.id,state:'unknown',reason:'response lost'});
      await page.locator('[data-a=answer-retry]').click();
      await page.waitForFunction(()=>messages.filter(m=>m.a==='answer').length===2);
      assert.deepEqual(await page.evaluate(()=>messages.filter(m=>m.a==='answer').map(m=>m.op)),[op,op]);
      await page.evaluate(r=>taskq.receive({answerReceipt:r}),{root,id:op.id,state:'accepted'});
      await page.waitForFunction(()=>document.querySelector('[role=status]').textContent.includes('accepted'));
      assert.equal(await page.locator('.qac').count(),0);
      assert.equal(await page.locator('.qac textarea').count(),0);
      assert.equal(await page.locator('.tqp-task a[data-url]').first().getAttribute('href'),board+'/'+rows[1].n);
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
      const ask={...rows[1],n:42,state:'ask',card:'Reviewer cannot continue.',question:{parsed:false,text:'Reviewer cannot continue.',choices:[]}};
      state.projects[0].tasks=[ask,...['doing','review','ready','later','waiting'].map((state,i)=>({...rows[1],n:50+i,state}))];
      await page.evaluate(s=>taskq.receive(s),state);
      assert.equal(await page.locator('.qac').count(),1);
      const reason=page.getByRole('textbox',{name:'Requeue reason'});
      assert(await page.getByRole('button',{name:'Requeue',exact:true}).isDisabled());
      await reason.fill('Retry after fixing the check');
      await page.getByRole('button',{name:'Requeue',exact:true}).click();
      await page.waitForFunction(()=>messages.filter(m=>m.a==='answer').length===3);
      const fallback=await page.evaluate(()=>messages.filter(m=>m.a==='answer').at(-1).op);
      assert.equal(fallback.action,'requeue');assert.equal(fallback.text,'Retry after fixing the check');
      await page.evaluate(r=>taskq.receive({answerReceipt:r}),{root,id:fallback.id,state:'rejected',reason:'test refusal'});
      await page.getByRole('button',{name:'Close…',exact:true}).click();
      assert.equal(await page.evaluate(()=>messages.filter(m=>m.a==='answer').length),3,'opening confirmation writes nothing');
      await page.getByRole('button',{name:'Keep task',exact:true}).click();
      assert.equal(await page.getByRole('button',{name:'Close without publishing',exact:true}).count(),0);
      await page.getByRole('button',{name:'Close…',exact:true}).click();
      await page.getByRole('button',{name:'Close without publishing',exact:true}).focus();
      await page.keyboard.press('Enter');
      await page.waitForFunction(()=>messages.filter(m=>m.a==='answer').length===4);
      const cancel=await page.evaluate(()=>messages.filter(m=>m.a==='answer').at(-1).op);
      assert.equal(cancel.action,'close');assert.equal(cancel.confirmed,true);
      state.projects[0].tasks[0]={...ask,state:'later'};await page.evaluate(s=>taskq.receive(s),state);
      await page.evaluate(r=>taskq.receive({answerReceipt:r}),{root,id:cancel.id,state:'unknown',reason:'Parked; close not verified'});
      assert.equal(await page.locator('.qac').count(),0);
      assert((await page.locator('[role=status]').allTextContents()).some(t=>t.includes('Parked; close not verified')));
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
      assert.deepEqual(errors,[]);
      console.log('PASS tray HTML draft/focus, stale review, literal text, sending refresh, same-ID retry, accepted readback, board link;',theme,font+'px');
      await page.close();
    }
  }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exit(1);});
