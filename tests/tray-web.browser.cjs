// Risk: an owned async callback records a failure but leaves apparently usable controls on screen.
const fs=require('node:fs'),assert=require('node:assert/strict'),{chromium}=require('playwright');
(async()=>{
 const browser=await chromium.launch(require('./browser.cjs')());
 try{
  const page=await browser.newPage();
  await page.addInitScript(()=>{window.messages=[];window.ipc={postMessage:text=>messages.push(JSON.parse(text))};});
  await page.goto('about:blank');
  await page.setContent(fs.readFileSync('build/web/tray.html','utf8'));
  await page.evaluate(()=>{
    __coreqExecution.effect('answer.send',()=>1,()=>{throw new Error('PRIVATE CALLBACK ERROR');});
  });
  await page.waitForFunction(()=>document.querySelector('[role=alert]')?.textContent.includes('command outcomes unknown'),{timeout:3000});
  assert.equal(await page.locator('.menu').evaluate(el=>getComputedStyle(el).display),'none');
  const messages=await page.evaluate(()=>window.messages);
  assert(messages.some(m=>m.a==='web-error'&&m.counts[0]===1));
  assert(messages.some(m=>m.a==='web-observation'&&m.row.operation==='js.answer.send.result'&&m.row.outcome==='failed'));
  assert(!JSON.stringify(messages).includes('PRIVATE CALLBACK ERROR'),'fault metadata leaked the exception text');
  console.log('PASS actual owned callback failure -> platform reporter -> independent interface fault and safe metadata');
 }finally{await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
