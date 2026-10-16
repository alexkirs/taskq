// Inventory and architecture checks only; never rewrites JavaScript.
'use strict';
const fs=require('node:fs'),path=require('node:path'),acorn=require('acorn');
const root=path.resolve(__dirname,'..'),operations=new Set(['js.ui.timer','js.collector.diagnostic','js.collector.health']);
const events=['scroll','pointermove','pointerdown','pointerup','pointercancel','lostpointercapture','blur','click','input','focusout','mousedown','keydown','toggle','submit','change'];
for(const event of events){operations.add('js.ui.'+event);operations.add('js.ui.events.'+event);}
for(const name of ['tray-app','answer-card','resource-controls','project-blocks','finding-composer']){
 const source=fs.readFileSync(path.join(root,'src',name+'.js'),'utf8');
 const tree=acorn.parse(source,{ecmaVersion:'latest'});
 function visit(node){
  if(node.type==='FunctionDeclaration'&&!['answerSend','resourceSend'].includes(node.id.name))throw Error(name+': ordinary methods must use execution.module: '+node.id.name);
  if(node.type==='VariableDeclarator'&&node.init&&['ArrowFunctionExpression','FunctionExpression'].includes(node.init.type))throw Error(name+': named helpers must use execution.module');
  if(node.async||node.type==='AwaitExpression')throw Error(name+': async work needs one execution.effect owner');
  if(node.type==='CallExpression'&&node.callee.type==='MemberExpression'){
   const member=node.callee.property.name;
   if(node.callee.object.name==='execution'&&member==='module'){
    const [prefix,methods]=node.arguments;
    if(typeof prefix.value!=='string'||methods.type!=='ObjectExpression')throw Error(name+': static module inventory required');
    for(const method of methods.properties)operations.add('js.'+prefix.value+'.'+(method.key.name||method.key.value));
   }
   if(['effect','action'].includes(member)&&['execution','ui'].includes(node.callee.object.name)){
    const op=node.arguments[0].value;
    if(typeof op!=='string')throw Error(name+': static effect/action name required');
    operations.add('js.'+op);if(member==='effect')operations.add('js.'+op+'.result');
   }
   if(['addEventListener','setTimeout','setInterval'].includes(member))throw Error(name+': use ui execution boundary: '+member);
  }
  for(const value of Object.values(node)){
   if(Array.isArray(value)){for(const child of value)if(child?.type)visit(child);}
   else if(value?.type)visit(value);
  }
 }
 visit(tree);
}
const out=process.argv[2];if(out){fs.mkdirSync(out,{recursive:true});fs.writeFileSync(path.join(out,'operations.json'),JSON.stringify([...operations].sort()));}
else console.log('PASS UI module/effect inventory: '+operations.size+' operations');
