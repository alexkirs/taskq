// Import the actual controller; use its explicit state and view ports.
const tray=require('../src/tray-app.js');
function install(context){
 const state=tray.createState();
 for(const key of Object.keys(state)){
  if(Object.hasOwn(context,key))state[key]=context[key];
  Object.defineProperty(context,key,{configurable:true,enumerable:true,get:()=>state[key],set:value=>state[key]=value});
 }
 context.document??={scrollingElement:{scrollTop:0},activeElement:null,fonts:{ready:Promise.resolve()},addEventListener(){}};
 context.CSS??={escape:String};
 context.menu??={querySelector:()=>null,querySelectorAll:()=>[]};
 const app=tray.create(state,{window:context,document:context.document,menu:context.menu,send:m=>context.send?.(m),
  render:(...args)=>context.render?.(...args),measureScrollbar:context.gutter,
  timers:{defer:(f,ms)=>context.setTimeout?.(f,ms),cancel:t=>context.clearTimeout?.(t)}});
 Object.assign(context,app);return app;
}
module.exports={install};
