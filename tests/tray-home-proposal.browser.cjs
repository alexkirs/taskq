// Chain: #1115 Home bounded AI blocks after a reader scrolls them. Instrument with Playwright (Chromium); not native WKWebView proof.
// Risk: a scroll stops mid-line in the exchange or the proposed changes, or alignment hides the first or last line.
const fs=require('node:fs'),assert=require('node:assert/strict'),{chromium}=require('playwright');
const html=fs.readFileSync('build/web/tray.html','utf8');
const long=Array.from({length:30},(_,i)=>'Answer line '+(i+1)+' with enough words to wrap across the popover').join('\n');
const answer={state:'proposal',rev:2,text:long,ops:[{project:'storefront',op:'answer',task:41,text:'PM: '+Array.from({length:12},(_,i)=>'reason '+(i+1)+' for keeping the larger quantity').join('; ')}]};
const request={id:'101',root:'storefront',text:'Ask',answer,chain:[{text:'Older',answer:{state:'clarify',text:'Which project?'}},{text:'Ask',answer}]};
// edges of each bounded block that cut a text line, and whether its first and last lines are fully inside it
const probe=()=>[...document.querySelectorAll('[data-bound]')].map(c=>{
  const b=c.getBoundingClientRect(),top=b.top+c.clientTop,bottom=top+c.clientHeight,r=document.createRange(),w=document.createTreeWalker(c,NodeFilter.SHOW_TEXT),l=[];
  for(let n;(n=w.nextNode());){r.selectNodeContents(n);l.push(...r.getClientRects());}
  const cut=e=>l.some(x=>x.top<e-1&&x.bottom>e+1),inside=x=>x.top>=top-1&&x.bottom<=bottom+1;
  return {name:c.getAttribute('aria-label'),scrolls:c.scrollHeight>c.clientHeight,cut:cut(top)||cut(bottom),first:inside(l[0]),last:inside(l[l.length-1])};});
// Observe the real debounce callback; a visually aligned intermediate frame is not completion.
async function settled(page,at='') {
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
  await page.waitForFunction('([...document.querySelectorAll("[data-bound]")].every(c=>!c.settling||window.fixtureTimers.has(c.settling)))&&('+probe.toString()+')().every(b=>!b.cut'+(at==='end'?'&&b.last':at==='start'?'&&b.first':'')+')',null,{timeout:3000});
}
(async()=>{
  const browser=await chromium.launch(require('./browser.cjs')());
  try{
    for(const font of [16,20]){
      const page=await browser.newPage({viewport:{width:font===20?360:320,height:1800}});
      const errors=[];page.on('pageerror',e=>errors.push(e.message));
      await page.evaluate(()=>{
        window.ipc={postMessage(){}};window.fixtureTimers=new Set();const native=setTimeout.bind(window);
        window.setTimeout=(callback,delay,...args)=>{
          if(typeof callback!=='function')return native(callback,delay,...args);
          const id=native(()=>{try{callback.apply(window,args);}finally{window.fixtureTimers.add(id);}},delay);return id;
        };
      });
      await page.setContent(html);
      await page.evaluate(s=>taskq.receive(s),{page:'home',font,projects:[{root:'storefront',name:'storefront',alive:true,complete:true,focus:[],labels:[],tasks:[]}],ai:{target:'storefront',history:[],draft:'',request}});
      const blocks=await page.evaluate(probe);
      assert.deepEqual(blocks.map(b=>[b.name,b.scrolls]),[['AI exchange',true],['Proposed changes',true]],font+' px: both blocks bounded');
      for(const at of ['middle','end','start']){
        await page.evaluate(at=>document.querySelectorAll('[data-bound]').forEach(c=>{c.scrollTop={middle:c.scrollHeight/2-c.clientHeight/2+3,end:c.scrollHeight,start:0}[at];}),at);
        await settled(page,at);
        for(const b of await page.evaluate(probe)){
          assert(!b.cut,font+' px '+at+': '+b.name+' cuts a line');
          if(at==='end')assert(b.last,font+' px end: '+b.name+' hides its last line');
          if(at==='start')assert(b.first,font+' px start: '+b.name+' hides its first line');
        }
      }
      // chain.length == answer.rev: a refresh keeps each block's own scroll place
      const state={page:'home',font,projects:[{root:'storefront',name:'storefront',alive:true,complete:true,focus:[],labels:[],tasks:[]}],ai:{target:'storefront',history:[],draft:'',request}};
      await page.evaluate(()=>{const [x,o]=document.querySelectorAll('[data-bound]');x.scrollTop=0;o.scrollTop=o.scrollHeight;});
      await settled(page);
      const before=await page.evaluate(()=>[...document.querySelectorAll('[data-bound]')].map(c=>c.scrollTop));
      assert(before[0]!==before[1],font+' px: blocks sit at different scroll places');
      await page.evaluate(s=>taskq.receive(s),state);
      assert.deepEqual(await page.evaluate(()=>[...document.querySelectorAll('[data-bound]')].map(c=>c.scrollTop)),before,font+' px: receive keeps both scroll places');
      assert.deepEqual(errors,[]);await page.close();
    }
  }finally{await browser.close();}
  console.log('PASS Home AI blocks realign on whole lines after middle, end and start scrolls at 16 and 20 px');
})().catch(e=>{console.error(e);process.exit(1);});
