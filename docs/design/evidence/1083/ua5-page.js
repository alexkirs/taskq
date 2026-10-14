// Writes one page per theme/font: the real tray.html with the #1051 U-A5 fixture pushed to the Activity history route.
// Usage: node ua5-page.js <repo> <outdir>
const fs=require('fs'),path=require('path'),[repo,out]=process.argv.slice(2);
const src=n=>fs.readFileSync(path.join(repo,'src',n),'utf8');
let html=src('tray.html');
for(const [k,f] of [['ANSWER_CARD_CSS','answer-card.css'],['ANSWER_CARD_JS','answer-card.js'],['PROJECT_BLOCKS_CSS','project-blocks.css'],['PROJECT_BLOCKS_JS','project-blocks.js'],['RESOURCE_CONTROLS_CSS','resource-controls.css'],['RESOURCE_CONTROLS_JS','resource-controls.js']])
  html=html.replace('{{'+k+'}}',()=>src(f));
const end=1791996060,start=end-86400,bins=Array.from({length:12},()=>[0,0,0,0]);
bins[0]=[1,1,0,0];bins[6]=[1,1,0,1];bins[11]=[1,0,1,1];
for(const theme of ['light','dark'])for(const font of [14,16,18,20]){
  const S={platform:'windows',version:'candidate',host:'win',view:'icon',font,period:'1d',pinned:false,now:end,theme,page:'cadence',
    projects:[{root:'p',name:'taskq',host:'win',version:'candidate',alive:true,age:10,complete:true,since:start-86400,focus:['x'],labels:[],tasks:[]}],
    notify:{status:'ok',host:{},projects:{}},agents:null,ai:{target:'p',request:null,draft:'',history:[]},add:null,
    history:{start,end,tz:'UTC',bins,since:start-86400},settings_error:null};
  const boot='<script>window.ipc={postMessage(){}};onerror=(m)=>{document.title="ERROR "+m};addEventListener("load",()=>{taskq.receive('+JSON.stringify(S)+');'+
    'const m=document.querySelector(".hist>summary>.meta"),r=m.getBoundingClientRect();document.title=(document.title.startsWith("ERROR")?document.title:"")+" lines="+Math.round(r.height/parseFloat(getComputedStyle(m).lineHeight))+" right="+Math.round(r.right)+" vw="+innerWidth+" sw="+document.documentElement.scrollWidth+" ell="+(m.scrollWidth>m.clientWidth)+" text="+m.innerText.replace(/\\s+/g," ");});</script>';
  fs.writeFileSync(path.join(out,`ua5-${theme}-${font}.html`),html.replace('<head>','<head>'+boot));
}
