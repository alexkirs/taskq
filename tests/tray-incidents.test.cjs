const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const html=fs.readFileSync('src/tray.html','utf8');
const context={esc:s=>String(s).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('"','&quot;')};
vm.createContext(context);
vm.runInContext(html.slice(html.indexOf('function incident('),html.indexOf('function service(')),context);
for(const state of ['sent','local','unsupported-auth','disabled','none']){
  context.p={root:'fixture',name:'<project>',alive:true,incident:state==='none'?null:{state,path:'.taskq/incidents/report.json',url:'https://github.com/taskqorg/taskq/issues/1'},incident_line:'Incident fixture: '+state};
  const h=vm.runInContext('incident(p)',context),top=vm.runInContext('incident(p,true)',context);
  assert(h.includes('Incident fixture: '+state));
  const local=['local','unsupported-auth'].includes(state);
  assert.equal(h.includes('Open report'),local);assert.equal(h.includes('Report manually'),local);
  assert.equal(h.includes('Open incident'),state==='sent');assert.equal(!!top,local);
  if(local){assert(top.includes('&lt;project>'));assert(top.includes('incident-attention'));assert(!top.includes('class="body alert"'));}
  context.p.alive=false;context.p.age=400;
  assert(vm.runInContext('incident(p)',context).includes('unknown (stale status.json)'));
  assert.equal(vm.runInContext('incident(p,true)',context),'');
}
context.p={alive:false};assert(vm.runInContext('incident(p)',context).includes('unknown (no status.json)'));
context.p={alive:true};assert(vm.runInContext('incident(p)',context).includes('unknown (no incident status)'));
context.p={alive:true,incident:{state:'local',path:'.taskq/incidents/report.json'},incident_line:'Incident: local dispatch.panic (offline) · .taskq/incidents/report.json'};
const compact=vm.runInContext('incident(p)',context);
assert(compact.includes('title="Incident: local dispatch.panic (offline) · .taskq/incidents/report.json"'));
assert(compact.includes('>Incident: local dispatch.panic (offline)</div>'));
context.p={name:'fixture',incident_notice:true,incident_repo:'taskqorg/taskq'};
const notice=vm.runInContext('incidentNotice(p)',context);
for(const text of ['Incident reports: notice','taskqorg/taskq','public','Posting account: unknown','Reports stay local','incident_reports','false','TASKQ_INCIDENT_REPORTS=off'])assert(notice.includes(text));
assert(!notice.includes('data-a='),'disclosure has no action that could enable reporting');
context.p.incident_notice=false;assert.equal(vm.runInContext('incidentNotice(p)',context),'');
console.log('PASS five incident states, missing/stale status, local actions, escaped project, disclosure without automatic enable');
