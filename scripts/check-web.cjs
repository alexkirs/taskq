// Inventory and architecture checks only; never rewrites JavaScript.
'use strict';
const fs=require('node:fs'),path=require('node:path'),acorn=require('acorn');
function inventory(source,name){
 const operations=new Set(),wrapped=new WeakSet();
 const fail=reason=>{throw Error(name+': '+reason);};
 function visit(node,parent,events=new Map()){
  if(node.type==='ForOfStatement'&&node.left.type==='VariableDeclaration'&&node.right.type==='ArrayExpression'){
   events=new Map(events);events.set(node.left.declarations[0].id.name,node.right.elements.map(n=>n.value));
  }
  if(['ClassDeclaration','ClassExpression'].includes(node.type))fail('ordinary methods must use execution.module');
  if(node.type==='FunctionDeclaration'&&!(name==='tray-app.js'&&['answerSend','resourceSend'].includes(node.id.name)))fail('ordinary methods must use execution.module: '+node.id.name);
  if(node.type==='Property'&&(node.method||node.value.type==='FunctionExpression')&&!wrapped.has(parent))fail('object methods must use execution.module');
  if(node.type==='VariableDeclarator'&&node.init&&['ArrowFunctionExpression','FunctionExpression'].includes(node.init.type))fail('named helpers must use execution.module');
  if(node.async||node.type==='AwaitExpression')fail('async work needs one execution.effect owner');
  if(node.type==='CallExpression'){
   const member=node.callee.type==='MemberExpression'?node.callee.property.name:node.callee.name;
   if(['addEventListener','setTimeout','setInterval'].includes(member))fail('use ui execution boundary: '+member);
   if(node.callee.object?.name==='execution'&&member==='module'){
    const [prefix,methods]=node.arguments;
    if(typeof prefix.value!=='string'||methods.type!=='ObjectExpression')fail('static module inventory required');
    wrapped.add(methods);
    for(const method of methods.properties)operations.add('js.'+prefix.value+'.'+(method.key.name||method.key.value));
   }
   if(['effect','action'].includes(member)&&['execution','ui'].includes(node.callee.object?.name)){
    const op=node.arguments[0].value;if(typeof op!=='string')fail('static effect/action name required');
    operations.add('js.'+op);if(member==='effect')operations.add('js.'+op+'.result');
   }
   if(node.callee.object?.name==='ui'&&member==='listen'){
    const event=node.arguments[1],types=event.type==='Literal'?[event.value]:events.get(event.name);
    if(!types?.length||types.some(type=>typeof type!=='string'||!/^[a-z]+$/.test(type)))fail('static event type or literal event list required');
    for(const type of types){operations.add('js.ui.'+type);operations.add('js.ui.events.'+type);}
   }
  }
  for(const value of Object.values(node)){
   if(Array.isArray(value)){for(const child of value)if(child?.type)visit(child,node,events);}
   else if(value?.type)visit(value,node,events);
  }
 }
 visit(acorn.parse(source,{ecmaVersion:'latest'}));return operations;
}
if(require.main===module){
 const root=path.resolve(__dirname,'..'),operations=new Set(['js.ui.timer','js.collector.diagnostic','js.collector.health']);
 // Capture/bootstrap infrastructure is deliberately outside recursive self-instrumentation.
 const infrastructure=new Set(['ui-runtime.js','tray-web.js','tray-entry.js','tray-guard.js']);
 for(const file of fs.readdirSync(path.join(root,'src'),{recursive:true}).filter(file=>file.endsWith('.js')&&!infrastructure.has(file)))
  for(const operation of inventory(fs.readFileSync(path.join(root,'src',file),'utf8'),file))operations.add(operation);
 const out=process.argv[2];if(out){fs.mkdirSync(out,{recursive:true});fs.writeFileSync(path.join(out,'operations.json'),JSON.stringify([...operations].sort()));}
 else console.log('PASS UI module/effect inventory: '+operations.size+' operations');
}
module.exports=inventory;
