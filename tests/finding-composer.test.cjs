// Chain F1/F2: reviewed draft -> bounded immutable finding CLI payload -> truthful separate receipts.
// Risks: producer ownership inference, leaked extra source fields, changed start choice, malformed success.
const assert=require('node:assert/strict'),fs=require('node:fs'),crypto=require('node:crypto');
const q=require('../src/finding-composer.js'),fx=require('./fixtures/finding-composer.json');
const input={...fx.finding,rawLogs:'never attach',path:'never attach',instructions:'never execute'},p=fx.projects[0],d=q.fresh(input),cap={create:true,start:true};
assert.equal(d.start,false);assert.equal(d.reviewed,false);
assert.equal(q.fresh({...input,mapping:undefined}).project,'');
assert.equal(q.fresh({...input,mapping:{project:p.id}}).project,'');
assert.match(q.problem(input,d,p,cap),/Review/);d.reviewed=true;assert.equal(q.problem(input,d,p,cap),null);
const op=q.request(input,d,p);assert.deepEqual(Object.keys(op),['board','source','title','goal','acceptance','evidence','start']);
assert.equal(op.start,false);assert.equal(op.source,input.source);assert(!JSON.stringify(op).includes('never'));
d.title='Changed';d.evidence='Changed';assert.equal(op.title,input.title);assert.equal(op.evidence[0],input.sanitizedEvidence);
assert.match(q.problem(input,d,p,{create:false,reason:'Denied'}),/Denied/);
assert.match(q.problem({...input,revision:2},d,p,cap),/Source changed/);
d.start=true;assert.match(q.problem(input,d,p,{create:true,start:false}),/start is unavailable/);
assert.match(q.problem({...input,source:'../rule/episode'},d,p,cap),/Invalid source/);
d.evidence='x'.repeat(239);assert.match(q.problem(input,d,p,cap),/238/);
for(const value of [null,{}, {create:{outcome:'accepted'}},{create:{outcome:'accepted',task:0}}])assert.equal(q.receipt(value).create.outcome,'unknown');
assert.deepEqual(q.receipt(fx.receipts.partial),fx.receipts.partial);
assert.equal(q.receipt({...fx.receipts.held,start:{outcome:'accepted',task:2402}}).start.outcome,'unknown');
const manifest=JSON.parse(fs.readFileSync('docs/design/finding-composer.sources.json','utf8'));
for(const [file,hash] of Object.entries(manifest.files))assert.equal(crypto.createHash('sha256').update(fs.readFileSync(file,'utf8').replace(/\r\n/g,'\n')).digest('hex'),hash,file);
const release=fs.readFileSync('.github/workflows/release.yml','utf8');
for(const file of ['build/web/finding-composer.js','src/finding-composer.css','tests/fixtures/finding-composer.json','docs/design/finding-composer.sources.json'])assert(release.includes('cp '+file));
console.log('PASS finding review, ownership, evidence allowlist, bounds, immutable payload, separate receipts and provenance');
