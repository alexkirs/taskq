const { chromium }=require('playwright');
const assert=require('node:assert/strict');
const path=require('node:path');
// Evidence PNGs only with --evidence; a plain run writes nothing.
const ev=(...p)=>process.argv.includes('--evidence')?path.join(__dirname,...p):undefined;
(async()=>{
 const browser=await chromium.launch({headless:true, channel:'chrome'});const page=await browser.newPage({viewport:{width:1024,height:1050},deviceScaleFactor:1});const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto('file://'+path.join(__dirname,'menu-next-preview.html'));
 const f=page.frameLocator('iframe');await f.locator('#tqnext .menu').first().waitFor();
 const menu=f.locator('[data-layout=menu]');
 await menu.locator('[data-action=stop]').click();await menu.locator('[data-action=stop-confirm]').click();
 await menu.locator('[data-restart]').check();await menu.locator('[data-action=restart-review]').click();await menu.locator('[data-action=restart-confirm]').click();
 assert.match(await menu.innerText(),/Recovery applied/);assert.match(await menu.innerText(),/Agent liveness unknown/);
 await page.screenshot({path:ev('evidence/menu-next/recovery-light.png'),fullPage:true});
 await menu.locator('[data-action=home]').first().click();await menu.locator('[data-action=pause]').click();assert.match(await menu.innerText(),/Recovery applied/);
 await menu.locator('[data-action=home]').first().click();await menu.locator('[data-action=work]').first().click();await menu.locator('[data-action=rule][data-value=snapshot]').click();await menu.locator('[data-action=scope-review]').click();assert.match(await menu.innerText(),/Only the selected 4 tasks/);
 await menu.locator('[data-action=scope-apply]').click();assert.match(await menu.innerText(),/admin, no billing/);
 const frame=page.frames().find(x=>x!==page.mainFrame());
 const model=await frame.evaluate(()=>({a:TaskqNextModel.canRestart(true,true,true),b:TaskqNextModel.canRestart(true,false,true),c:TaskqNextModel.canRestart(true,true,false),totals:TaskqNextModel.binTotals}));assert.equal(model.a,true);assert.equal(model.b,false);assert.equal(model.c,false);assert.equal(model.totals.length,4);
 await frame.evaluate(()=>{document.querySelector('[data-variant="Menu review"]').hidden=true;document.querySelector('[data-variant="Workspace review"]').hidden=false});
 const ws=f.locator('[data-layout=workspace]');await ws.locator('[data-action=work]').first().click();await page.emulateMedia({colorScheme:'dark'});await page.waitForTimeout(250);
 await page.screenshot({path:ev('evidence/menu-next/workspace-dark.png'),fullPage:true});
 await page.setViewportSize({width:360,height:1500});await f.locator('[data-preview=font]').selectOption('20');
 const overflow=await frame.evaluate(()=>{const r=document.getElementById('tqnext');return {client:r.clientWidth,scroll:r.scrollWidth}});assert.ok(overflow.scroll<=overflow.client+1,JSON.stringify(overflow));
 await page.screenshot({path:ev('evidence/menu-next/narrow-dark-20.png'),fullPage:true});
 assert.deepEqual(errors,[]);await browser.close();console.log('PASS: stop/recovery guard, scope snapshot review, no page errors, narrow 20px width');
})().catch(e=>{console.error(e);process.exit(1)});
