// Design-model checks only. No backend or native-platform qualification.
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const path = require('node:path');
const html = fs.readFileSync(path.join(__dirname, 'final-prototype.html'), 'utf8');
const ctx = {};
vm.runInNewContext(html.match(/<script>([\s\S]*?)<\/script>/)[1], ctx);
const m = ctx.taskqDesignModel;
const c = {id:'a', rev:2, state:'proposal'};
assert.equal(m.confirm(c,'b',2), false, 'another conversation cannot confirm');
assert.equal(m.confirm(c,'a',1), false, 'stale revision cannot confirm');
assert.equal(m.confirm(c,'a',2), true);
assert.equal(m.confirm(c,'a',2), false, 'duplicate confirmation cannot apply');
assert.equal(m.sessionLink({host:'remote', local:false, url:'codex://x'}),false);
assert.equal(m.sessionLink({host:'Mac', local:true, url:null}),false);
assert.equal(m.sessionLink({host:'Mac', local:true, url:'codex://recorded'}),true);
const owners={};
assert.equal(m.openWindow(owners,'a',false),'main');
assert.equal(m.openWindow(owners,'a',true),'detached');
assert.equal(m.openWindow(owners,'a',false),'detached', 'reopen raises owner');
assert.equal(Object.keys(owners).length,1);
assert.equal(m.reattach(owners,'a'),'main');
assert.equal(Object.keys(owners).length,0);
console.log('PASS: conversation/revision isolation, repeat confirmation, local session eligibility, window ownership');
const tasks=[{provider:'github',server:'github.com',repo:'a',id:1,state:'doing'},
 {provider:'github',server:'github.com',repo:'a',id:1,state:'doing'},
 {provider:'github',server:'github.com',repo:'a',id:2,state:'review'},
 {provider:'gitlab',server:'gitlab.example',repo:'a',id:1,state:'failed'},
 {provider:'github',server:'github.com',repo:'a',id:3,state:'closed'}];
const counts=m.countTasks(tasks);
assert.equal(counts.doing,1);assert.equal(counts.review,1);assert.equal(counts.failed,1);assert.equal(counts.closed,1);
assert.equal(counts.ask,0);
tasks[4].state='ready';assert.equal(m.countTasks(tasks).closed,0);
console.log('PASS: counter identity, disjoint states and reopened Closed task');
