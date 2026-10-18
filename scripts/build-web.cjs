// Standard JS bundles; no parser changes product source.
'use strict';
const fs=require('node:fs'),path=require('node:path'),esbuild=require('esbuild');
const root=path.resolve(__dirname,'..');
const out=path.resolve(process.argv[2]||path.join(root,'build/web'));
const cargo=fs.readFileSync(path.join(root,'Cargo.toml'),'utf8');
const revisions=[...cargo.matchAll(/coreq(?:-instrument)? = \{ git = "https:\/\/github.com\/taskqorg\/coreq", rev = "([a-f0-9]{40})"/g)].map(m=>m[1]);
const dependency=JSON.parse(fs.readFileSync(path.join(root,'package.json'),'utf8')).dependencies['@taskq/coreq'];
if(revisions.length!==2||revisions[0]!==revisions[1]||dependency!=='https://codeload.github.com/taskqorg/coreq/tar.gz/'+revisions[0])throw Error('Cargo/instrumenter/JS must pin the same CoreQ revision');

fs.mkdirSync(out,{recursive:true});
require('node:child_process').execFileSync(process.execPath,[path.join(__dirname,'check-web.cjs'),out],{stdio:'inherit'});
const components={'answer-card':'TaskqAnswerCard','resource-controls':'TaskqResources','project-blocks':'TaskqProjectBlocks','finding-composer':'TaskqFindingComposer'};
for(const [name,globalName] of Object.entries(components))esbuild.buildSync({entryPoints:[path.join(root,'src',name+'.js')],bundle:true,platform:'browser',format:'iife',globalName,outfile:path.join(out,name+'.js'),target:'es2022'});
for(const name of ['tray-guard','tray-entry'])esbuild.buildSync({entryPoints:[path.join(root,'src',name+'.js')],bundle:true,platform:'browser',format:'iife',outfile:path.join(out,name+'.js'),target:'es2022'});
let html=fs.readFileSync(path.join(root,'src/tray.html'),'utf8');
for(const name of ['answer-card','project-blocks','resource-controls'])html=html.replace('{{'+name.toUpperCase().replaceAll('-','_')+'_CSS}}',fs.readFileSync(path.join(root,'src',name+'.css'),'utf8'));
for(const [key,name] of [['GUARD_JS','tray-guard'],['APP_JS','tray-entry']])html=html.replace('{{'+key+'}}',fs.readFileSync(path.join(out,name+'.js'),'utf8').replaceAll('</script','<\/script'));
if(/\{\{[A-Z_]+\}\}/.test(html))throw Error('Unresolved web asset');
fs.writeFileSync(path.join(out,'tray.html'),html);
