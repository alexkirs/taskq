// Chain: service tick -> receive -> render while a button is pressed. Risk: the rebuilt menu drops the click
// (U-A4), a press never released stops refreshes, or the deferred refresh runs before the click.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const html=fs.readFileSync('src/tray.html','utf8');
const on={},win={},timers=[];let renders=0;
const context={render:()=>renders++,setTimeout:f=>timers.push(f),performance:{now:()=>0},
  menu:{addEventListener:(t,f)=>{on[t]=f;}},addEventListener:(t,f)=>{win[t]=f;}};
vm.createContext(context);
vm.runInContext(html.slice(html.indexOf('// a refresh between press and release'),html.indexOf('// Press end.')),context);
const run=s=>vm.runInContext(s,context);
run('refresh(0)');assert.equal(renders,1,'no press: a tick renders');
context.performance.now=()=>1000;on.pointerdown();
run('refresh(1500)');assert.equal(renders,1,'a tick during a press waits');
win.pointerup();assert.equal(renders,1,'the release does not rebuild before the click');
assert.equal(timers.length,1);timers.shift()();assert.equal(renders,2,'the waiting tick renders after the click');
win.pointerup();assert.equal(timers.length,0,'a release without a waiting tick renders nothing');
on.pointerdown();run('refresh(6001)');assert.equal(renders,3,'a press older than 5 s no longer holds refreshes');
on.pointerdown();run('refresh(1200)');win.blur();timers.shift()();assert.equal(renders,4,'leaving the window ends the press');
for(const event of ['pointerup','pointercancel'])assert(event in win);
