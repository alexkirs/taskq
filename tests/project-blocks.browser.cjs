// B4 instrument: identical parent/standalone/tray fixtures; browser output is not native or live-board proof.
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict'),crypto=require('node:crypto'),{chromium}=require('playwright');
const q=require('../src/project-blocks.js'),fx=require('./fixtures/project-blocks.json');
const source=fs.readFileSync('docs/design/menu-next-variants.html','utf8'),css=fs.readFileSync('src/project-blocks.css','utf8'),js=fs.readFileSync('src/project-blocks.js','utf8');
const parentCSS=[...source.matchAll(/<style>([\s\S]*?)<\/style>/g)].map(m=>m[1]).join('\n');
const parentFunctions=source.split('\n').filter(l=>/^(const (esc|icon|button|ib)=|function (counts|projectRow|task)\()/.test(l)).join('\n');
const tray=fs.readFileSync('src/tray.html','utf8').replace('{{TRAY_WEB_JS}}',fs.readFileSync('src/tray-web.js','utf8')).replace('{{ANSWER_CARD_JS}}',fs.readFileSync('src/answer-card.js','utf8')).replace('{{ANSWER_CARD_CSS}}',fs.readFileSync('src/answer-card.css','utf8')).replace('{{PROJECT_BLOCKS_JS}}',js).replace('{{PROJECT_BLOCKS_CSS}}',css).replace('{{RESOURCE_CONTROLS_JS}}',fs.readFileSync('src/resource-controls.js','utf8')).replace('{{RESOURCE_CONTROLS_CSS}}',fs.readFileSync('src/resource-controls.css','utf8'));
const base='<style>:root{color-scheme:light dark}body{margin:0;padding:8px;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}</style>';
function fixture(n){return {...fx.project,name:'project',closed:n,tasks:q.states.filter(s=>s[0]!=='closed').flatMap(([state],i)=>Array.from({length:n},(_,j)=>({...fx.row,n:i*1000+j+1,state,revision:4}))) };}
function look(sel){
  const root=document.querySelector(sel),strip=root.querySelector('.tqp-counts,.counts'),b=strip&&strip.querySelector('button'),g=(el,k)=>getComputedStyle(el)[k];
  if(!b)return null;
  return {color:g(root,'color'),fill:g(b,'backgroundColor'),radius:g(b,'borderTopLeftRadius'),padding:g(b,'padding'),height:g(b,'minHeight'),gap:g(strip,'gap'),wrap:g(strip,'flexWrap'),align:g(strip,'justifyContent'),closedFill:g(strip.lastElementChild,'backgroundColor'),closedText:g(strip.lastElementChild,'color')};
}
function geometry(sel){
  const groups=[...document.querySelectorAll(sel)],results=[];
  for(const strip of groups){
    const right=strip.getBoundingClientRect().right,lines=new Map();
    for(const button of strip.querySelectorAll('button')){const r=button.getBoundingClientRect();lines.set(Math.round(r.top),Math.max(lines.get(Math.round(r.top))||0,r.right));}
    results.push({lines:lines.size,rightGaps:[...lines.values()].map(x=>Math.round((right-x)*100)/100),states:[...strip.querySelectorAll('button')].map(b=>b.dataset.state||b.dataset.value)});
  }
  return {strips:results,overflow:document.documentElement.scrollWidth>innerWidth};
}
function taskLook(sel){const el=document.querySelector(sel),s=getComputedStyle(el);return {display:s.display,padding:s.padding,font:s.fontSize,lineHeight:s.lineHeight,wrap:s.overflowWrap,width:Math.round(el.getBoundingClientRect().width)};}
(async()=>{
  const browser=await chromium.launch(require('./browser.cjs')()),evidence=[];
  const dir='docs/design/evidence/project-blocks';if(process.argv.includes('--evidence'))fs.mkdirSync(dir,{recursive:true});
  try{
    for(const theme of fx.themes)for(const font of fx.fonts)for(const width of fx.widths)for(const n of fx.counts){
      const at={theme,font,width,count:n},p=fixture(n),pages=[];
      try{
        const parent=await browser.newPage({viewport:{width,height:1800},colorScheme:theme});pages.push(parent);
        await parent.setContent(base+'<style>'+parentCSS+'</style><div id="tqnext" style="font-size:'+font+'px"><div id="parent-project"></div><div id="parent-aggregate"></div><div id="parent-task"></div></div>');
        await parent.addScriptTag({content:'const preview={platform:"Windows"};const projectCounts='+JSON.stringify({[p.name]:q.states.map(([s])=>s==='closed'?n:n)})+';'+parentFunctions+';document.querySelector("#parent-project").innerHTML=projectRow('+JSON.stringify(p.name)+');document.querySelector("#parent-aggregate").innerHTML=counts('+JSON.stringify(p.name)+');document.querySelector("#parent-task").innerHTML=task({},938,'+JSON.stringify(fx.row.title)+');'});
        await parent.evaluate(assets=>{for(const i of document.querySelectorAll('[data-lucide]'))i.outerHTML=(assets[i.dataset.lucide]||'').replace('<svg','<svg aria-hidden="true"');},q.assets);
        const want=await parent.evaluate(look,'#parent-project');
        const wantTask=await parent.evaluate(taskLook,'#parent-task .task');
        for(const mode of ['browser','tray']){
          const page=await browser.newPage({viewport:{width,height:1800},colorScheme:theme});pages.push(page);
          const errors=[];page.on('pageerror',e=>errors.push(e.message));
          await page.setContent(mode==='tray'?tray:base+'<style>'+css+'</style><div id="consumer" style="font-size:'+font+'px"></div><script>'+js+'</script>');
          await page.evaluate(()=>{window.navigation=[];window.messages=[];window.ipc={postMessage:s=>messages.push(JSON.parse(s))};});
          if(mode==='browser')await page.evaluate(({p,row})=>{
            const q=TaskqProjectBlocks,el=document.querySelector('#consumer');el.innerHTML=q.summary(p)+q.aggregate([p])+q.task(p,row);
            el.addEventListener('click',e=>q.handle(e,{project:id=>navigation.push(['project',id]),tasks:(id,s)=>navigation.push(['tasks',id,s]),open:u=>navigation.push(['open',u])}));
          },{p,row:fx.row});
          if(mode==='tray')await page.evaluate(({p,row,font,theme})=>taskq.receive({page:p.id,font,theme,projects:[{...p,root:p.id,issues:p.board.url,board_kind:p.board.kind,alive:true,focus:[],labels:[],cap_saved:{error:'fixture'},tasks:p.tasks}],ai:{draft:''},notify:{projects:{}}}),{p,row:fx.row,font,theme});
          const got=await page.evaluate(look,'.tqp-project');assert.deepEqual(got,want,JSON.stringify(at)+' '+mode+' project equals parent');
          if(mode==='browser'||n)assert.deepEqual(await page.evaluate(taskLook,'.tqp-task'),wantTask,JSON.stringify(at)+' '+mode+' flowing task geometry equals parent');
          const geo=await page.evaluate(geometry,'.tqp-counts');assert.equal(geo.overflow,false,JSON.stringify(at)+' '+mode);
          for(const strip of geo.strips){assert(strip.rightGaps.every(g=>Math.abs(g)<1),JSON.stringify(at)+' '+mode+' K-A8');assert.deepEqual(strip.states,n?q.states.map(s=>s[0]):[]);}
          if(n)assert.equal(await page.locator('.tqp-project .tqp-counts button').count(),8);else assert.equal(await page.locator('.tqp-counts').count(),0);
          assert.deepEqual(errors,[],JSON.stringify(at)+' '+mode);
          evidence.push({...at,mode,look:got,taskLook:wantTask,geometry:geo});
          if(process.argv.includes('--evidence')&&font===20&&width===320&&n===100)await page.locator('.tqp-project').screenshot({path:path.join(dir,mode+'-'+theme+'-20-320.png')});
        }
        const parentGeo=await parent.evaluate(geometry,'.counts');assert.equal(parentGeo.overflow,false);for(const strip of parentGeo.strips)assert(strip.rightGaps.every(g=>Math.abs(g)<1));
      }finally{for(const p of pages)await p.close();}
    }
    // Real DOM navigation, long-flow layout, unknown/partial and GitLab fixtures for both consumers.
    for(const mode of ['browser','tray']){
      const page=await browser.newPage({viewport:{width:320,height:1800},colorScheme:'dark'});
      try{
        const p={...fixture(1),name:fx.project.name,closed:null},row={...fx.row,state:'review'};
        await page.setContent(mode==='tray'?tray:base+'<style>'+css+'</style><div id="consumer" style="font-size:20px"></div><script>'+js+'</script>');
        await page.evaluate(()=>{window.navigation=[];window.messages=[];window.ipc={postMessage:s=>messages.push(JSON.parse(s))};});
        if(mode==='browser')await page.evaluate(({p,row})=>{const el=document.querySelector('#consumer');el.innerHTML=TaskqProjectBlocks.summary(p)+TaskqProjectBlocks.task(p,row);el.addEventListener('click',e=>TaskqProjectBlocks.handle(e,{project:id=>navigation.push(['project',id]),tasks:(id,s)=>navigation.push(['tasks',id,s]),open:u=>navigation.push(['open',u])}));},{p,row});
        else await page.evaluate(({p,row})=>taskq.receive({page:p.id,font:20,theme:'dark',projects:[{...p,root:p.id,issues:p.board.url,board_kind:'github',alive:true,focus:[],labels:[],tasks:[row],cap_saved:{error:'fixture'}}],ai:{draft:''},notify:{projects:{}}}),{p,row});
        await page.locator('[data-tqp=tasks][data-state=review]').first().click();
        if(mode==='browser')assert.deepEqual(await page.evaluate(()=>navigation),[['tasks',p.id,'review']]);
        else {assert.match(await page.locator('.menu').innerText(),/project-with-a-long-name.* · Review/);assert.equal(await page.locator('.tqp-task').count(),1);}
        await page.locator('[data-tqp=project]').first().click();
        if(mode==='browser')assert.deepEqual(await page.evaluate(()=>navigation.at(-1)),['project',p.id]);
        await page.locator('.tqp-task a').first().click();await page.locator('.tqp-task a[aria-label="Open codex session"]').click();
        const opened=await page.evaluate(mode=>mode==='browser'?navigation.filter(n=>n[0]==='open').map(n=>n[1]):messages.filter(n=>n.a==='open').map(n=>n.url),mode);
        assert.deepEqual(opened,[row.url,row.session_url]);
        const flow=await page.locator('.tqp-task').evaluate(el=>({display:getComputedStyle(el).display,title:el.textContent,width:el.getBoundingClientRect().width}));
        assert.equal(flow.display,'block');assert(flow.title.includes(row.title));assert(flow.width>280);
        await page.evaluate(p=>{document.querySelector('.tqp-project').outerHTML=TaskqProjectBlocks.summary({...p,complete:false,board:{kind:'gitlab',url:'https://gitlab.example/o/r/-/issues'}});},p);
        assert.match(await page.locator('.tqp-project').innerText(),/Partial:/);assert.equal(await page.locator('.tqp-project [data-icon=gitlab]').count(),1);assert.equal(await page.locator('.tqp-project button[data-state]').count(),8);
        assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
      }finally{await page.close();}
    }
    // #1041: monochrome provider marks follow the theme ink in both consumers; explicitly filled marks keep their colors.
    for(const theme of fx.themes)for(const mode of ['browser','tray']){
      const page=await browser.newPage({viewport:{width:320,height:1800},colorScheme:theme});
      try{
        await page.setContent(mode==='tray'?tray:base+'<style>'+css+'</style><script>'+js+'</script>');
        if(mode==='tray')await page.evaluate(theme=>taskq.receive({page:'none',font:20,theme,projects:[],ai:{draft:''},notify:{projects:{}}}),theme);
        const marks=await page.evaluate(()=>{
          const q=TaskqProjectBlocks,p=k=>({id:k,name:k,board:{kind:k,url:'https://'+k+'.example/o/r/issues'},complete:true,closed:0,tasks:[]});
          const el=document.createElement('div');el.id='providers';el.style.cssText='font-size:20px;padding:8px;background:'+getComputedStyle(document.body).backgroundColor;document.body.prepend(el);
          el.innerHTML=q.summary(p('github'))+q.summary(p('gitlab'))+['claude','codex'].map((runtime,i)=>q.task(p('github'),{n:i+1,state:'doing',revision:1,title:runtime,url:'https://github.example/o/r/issues/'+(i+1),runtime})).join('');
          const color=svg=>getComputedStyle(svg).color;
          return ['github','gitlab','claude','openai'].map(name=>{const svg=el.querySelector('[data-icon='+name+']');return {name,fill:getComputedStyle(svg).fill,ink:color(svg)};});
        });
        for(const m of marks)assert.equal(m.fill,m.ink,theme+' '+mode+' '+m.name+' mark uses theme ink');
        assert.equal(await page.evaluate(()=>{const el=document.querySelector('#providers');el.insertAdjacentHTML('beforeend','<span class="tqp"><svg fill="#d97757"><path/></svg></span>');return getComputedStyle(el.lastChild.firstChild).fill;}),'rgb(217, 119, 87)');
        await page.evaluate(()=>document.querySelector('#providers').lastChild.remove());
        if(process.argv.includes('--evidence'))await page.locator('#providers').screenshot({path:path.join(dir,'providers-'+mode+'-'+theme+'.png')});
      }finally{await page.close();}
    }
    if(process.argv.includes('--evidence'))fs.writeFileSync(path.join(dir,'comparison.json'),JSON.stringify({claim:'Browser parent/standalone/tray comparison only; native and released-asset qualification deferred',manifestSha256:crypto.createHash('sha256').update(fs.readFileSync('docs/design/project-blocks.sources.json','utf8').replace(/\r\n/g,'\n')).digest('hex'),cases:evidence},null,2)+'\n');
    console.log('PASS 64 parent/browser/tray comparisons, exact counter look/order, all wrapped right edges, 16/20 px, 320/360 px, light/dark, 0/1/12/100, navigation, long content, partial and providers; provider marks readable in both themes');
  }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exit(1);});
