// Chain: WS11 menu -> settings/project/task routes -> Back. Risk: a refresh changes route, Back loses context,
// or moving maintenance hides existing actions. DOM/native rendering is qualified separately by snapshots.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const html=fs.readFileSync('src/tray.html','utf8');
const context={page:'home',taskFilter:null,working:null,workCard:null,ask:null,esc:s=>String(s),render:()=>{},
  document:{scrollingElement:{scrollTop:120}},menu:{querySelector:()=>null,querySelectorAll:()=>[{open:true},{open:false}]} };
vm.createContext(context);
vm.runInContext(html.slice(html.indexOf('// Navigation is view state'),html.indexOf('// Task answers:')),context);
const run=s=>vm.runInContext(s,context);
run("navigate('settings');navigate('appearance')");
assert.equal(context.page,'appearance');assert.equal(context.document.scrollingElement.scrollTop,0);
run('back()');assert.equal(context.page,'settings');
run('back()');assert.equal(context.page,'home');assert.equal(context.document.scrollingElement.scrollTop,120);
assert.equal(run('JSON.stringify(disclosures)'),'[true,false]');
run("navigate('project');navigate('project','review');navigate('project',null,{project:true})");
assert.equal(context.working.project,true);
run('back()');assert.equal(context.page,'project');assert.equal(context.taskFilter,'review');
run('back()');assert.equal(context.page,'project');assert.equal(context.taskFilter,null);
run('back();back()');assert.equal(context.page,'home');
// Render all five categories from the same snapshot, including empty and unknown states.
context.S={platform:'windows',font:20,host:'mac',now:1000,version:'candidate',projects:[],agents:null};
Object.assign(context,{history:()=>'<history>',workEntry:()=>'<work>',projects:()=>'<projects>',attention:()=>'',known:()=>true,
  update:()=>'<update>',age:n=>n+' s',problem:()=>'<problem>',incidentNotice:()=>'<notice>',service:()=>'<service>'});
run(html.slice(html.indexOf('function views(){'),html.indexOf('function attention(){')));
run(html.slice(html.indexOf('function routeBody(){'),html.indexOf('// Capacity (')));
context.page='settings';let h=run('routeBody()');
for(const route of ['appearance','cadence','notifications','agents','services','about'])assert(h.includes('data-route="'+route+'"'));
context.page='appearance';assert(run('routeBody()').includes('Text size · 20 px'));
context.page='cadence';assert.equal(run('routeBody()'),'<history>');
context.page='agents';assert(run('routeBody()').includes('Unknown: no status.json'));
context.S.projects=[{name:'alpha',host:'mac',version:null,age:null}];
context.page='services';h=run('routeBody()');assert(h.includes('Resources'));assert(h.includes('status unknown'));assert(h.includes('<problem><notice><service>'));
context.page='about';h=run('routeBody()');assert(h.includes('version unknown'));assert(h.includes('status unknown'));assert(h.includes('<update>'));
// M14: OS denial reads Blocked with the OS setting and no test post; a project shows its override and effective mode
run(html.slice(html.indexOf('// Notifications (menu.md M14)'),html.indexOf('// a notification click')));
const kinds={ask:'sound',close:'off',failed:'banner',gone:'banner'};
context.S.notify={status:'blocked',host:kinds,projects:{'/p':{ask:[null,'sound'],close:['banner','banner'],failed:[null,'banner'],gone:[null,'banner']}}};
context.page='notifications';h=run('routeBody()');
assert(h.includes('Blocked by the OS'));assert(h.includes('data-a="notify-os"'));assert.equal((h.match(/notify-test[^>]*disabled/g)||[]).length,4);
assert(h.includes('data-kind="ask" data-v="sound" aria-pressed="true"'));
context.S.notify.status='allowed';h=run('routeBody()');assert(!h.includes('notify-os'));assert(!/notify-test[^>]*disabled/.test(h));
h=run("notifyGroup({root:'/p'})");
assert(h.includes('data-kind="close" data-root="/p" data-v="banner" aria-pressed="true"'));assert(h.includes('data-kind="ask" data-root="/p" data-v="inherit" aria-pressed="true"'));
// M14 click on a closed task: no open row, so the project page shows its card from the notification until navigation
run(html.slice(html.indexOf('// a notification click'),html.indexOf('function project(p){')));
Object.assign(context,{TaskqProjectBlocks:require('../src/project-blocks.js'),CSS:{escape:s=>s},blockProject:p=>({name:p.name,board:{kind:'github',url:p.issues}}),
  S:{projects:[{root:'/p',name:'p',issues:'https://x/issues',tasks:[]}]}});
run("page='home';openCard({root:'/p',n:8,kind:'close',title:'Ship it'})");
assert.equal(context.page,'/p');h=run("notedCard(S.projects[0])");
assert(h.includes('data-card="https://x/issues/8"'));assert(h.includes('Closed'));assert(h.includes('Ship it'));assert(h.includes('href="https://x/issues/8"'));
run("navigate('home')");assert.equal(run("notedCard(S.projects[0])"),'');
console.log('PASS M14 notification settings, OS block, project override and closed-task card');
console.log('PASS WS11 nested Back, filter/scroll/disclosure restoration, settings categories, unknown probes/versions and maintenance destinations');

// Chain: M13 header/fit -> native size request -> reopened persisted dimensions.
// Risk: Top is coupled to Pin or a refresh overwrites the user's chosen size.
const sizing={devicePixelRatio:1.4,gutter:()=>8,S:{font:20,pinned:false,top:true,size:null},sent:[],document:{body:{style:{}}},menu:{style:{},getBoundingClientRect:()=>({height:421.4})},send:m=>sizing.sent.push(m)};
vm.createContext(sizing);
vm.runInContext(html.slice(html.indexOf('function fit(){'),html.indexOf('const menu=')),sizing);
vm.runInContext(html.slice(html.indexOf('function head(){'),html.indexOf('function render(')),sizing);
const sizeRun=s=>vm.runInContext(s,sizing);
sizeRun('fit()');assert.deepEqual(JSON.parse(JSON.stringify(sizing.sent.pop())),{a:'size',d:1.4,g:8,w:360,h:422});
assert(sizeRun('head()').includes('data-a="top" aria-pressed="true"'));
sizing.S.pinned=true;sizing.S.top=false;assert(sizeRun('head()').includes('data-a="top" aria-pressed="false"'));
sizing.S.size=[480,600];sizeRun('fit()');assert.equal(sizing.sent.at(-1).h,600);
sizeRun('manualSize=[500,300];fit()');assert.equal(sizing.sent.at(-1).w,500);assert.equal(sizing.sent.at(-1).h,300);
console.log('PASS M13 independent header toggles, measured initial height and preserved manual size');
