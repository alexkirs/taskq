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
for(const route of ['appearance','cadence','agents','services','about'])assert(h.includes('data-route="'+route+'"'));
context.page='appearance';assert(run('routeBody()').includes('Text size · 20 px'));
context.page='cadence';assert.equal(run('routeBody()'),'<history>');
context.page='agents';assert(run('routeBody()').includes('Unknown: no status.json'));
context.S.projects=[{name:'alpha',host:'mac',version:null,age:null}];
context.page='services';h=run('routeBody()');assert(h.includes('Resources'));assert(h.includes('status unknown'));assert(h.includes('<problem><notice><service>'));
context.page='about';h=run('routeBody()');assert(h.includes('version unknown'));assert(h.includes('status unknown'));assert(h.includes('<update>'));
console.log('PASS WS11 nested Back, filter/scroll/disclosure restoration, settings categories, unknown probes/versions and maintenance destinations');
