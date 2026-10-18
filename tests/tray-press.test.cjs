// Chain: actual press/release subscriptions preserve a click across service refresh.
const assert=require('node:assert/strict');
const on={},win={},timers=[];let renders=0,now=0;
const context={render:()=>renders++,setTimeout:f=>timers.push(f),performance:{now:()=>now},send(){},
 document:{fonts:{ready:Promise.resolve()},addEventListener(){}},
 menu:{addEventListener:(t,f)=>(on[t]??=[]).push(f)},addEventListener:(t,f)=>(win[t]??=[]).push(f)};
const app=require('./tray-harness.cjs').install(context);app.mount();
const event={button:0,target:{closest:()=>null}};
const fire=(surface,type)=>{for(const fn of surface[type])fn(event);};
app.refresh(0);assert.equal(renders,1);
now=1000;fire(on,'pointerdown');app.refresh(1500);assert.equal(renders,1);
fire(win,'pointerup');assert.equal(renders,1);assert.equal(timers.length,1);timers.shift()();assert.equal(renders,2);
fire(win,'pointerup');assert.equal(timers.length,0);
fire(on,'pointerdown');app.refresh(6001);assert.equal(renders,3);
fire(on,'pointerdown');app.refresh(1200);fire(win,'blur');timers.shift()();assert.equal(renders,4);
for(const event of ['pointerup','pointercancel'])assert(event in win);
console.log('PASS actual press callbacks, deferred refresh, stale press and blur release');
