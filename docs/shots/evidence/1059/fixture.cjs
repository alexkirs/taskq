// usage: node docs/shots/evidence/1059/fixture.cjs DIR STATE after docs/shots/fixture.sh DIR: projects file with native paths and a two-entry AI exchange (result|clarify|proposal|failed|long-proposal)
const fs=require('fs'),path=require('path');
const [d,st]=process.argv.slice(2),r=path.join(d,'storefront','.taskq','requests');
fs.writeFileSync(path.join(d,'install','projects'),['storefront','billing-api'].map(p=>path.join(d,p)).join('\n')+'\n');
fs.mkdirSync(r,{recursive:true});
const w=(n,v)=>fs.writeFileSync(path.join(r,n),JSON.stringify(v));
const long=Array.from({length:40},(_,i)=>'Answer line '+(i+1)+' of the long finished reply.').join('\n');
w('100.json',{text:'Which tasks wait for me?'});
w('100.answer.json',{id:'100',state:'clarify',text:'Which project do you mean?'});
w('101.json',{text:'storefront',after:'100'});
w('101.answer.json',{result:{id:'101',state:'result',text:long+'\nNext: 1 Answer #41 ★ · 2 Start #44'},
  clarify:{id:'101',state:'clarify',text:'Answer #41 now or later?'},
  proposal:{id:'101',state:'proposal',rev:2,text:'Plan ready.',ops:[{project:'storefront',op:'answer',task:41,option:1}]},
  failed:{id:'101',state:'failed',reason:'model timeout',text:long},
  'long-proposal':{id:'101',state:'proposal',rev:2,text:'PM comment on #1058 reviewed.\nNeeds an owner answer before the next attempt.',ops:[{project:'storefront',op:'answer',task:41,text:'PM: '+Array.from({length:12},(_,i)=>'reason '+(i+1)+' for keeping the larger quantity').join('; ')}]}}[st]);
