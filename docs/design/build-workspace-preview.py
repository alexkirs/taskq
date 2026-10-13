"""Compose the accepted workspace with the canonical settings component."""
from pathlib import Path
import re
import json
root = Path(__file__).parent
menu = (root / 'menu-next-variants.html').read_text()
ai = (root / 'ai-conversations.html').read_text()
aimarkup = ai.split('<style>')[0]
aiassets = ai[len(aimarkup):]
behavior=(root/'agent-behavior.html').read_text()
bmarkup=behavior.split('<style>')[0]
bassets=behavior[len(bmarkup):]
plans=(root/'plans-and-decisions.html').read_text()
pm=plans.split('<style>')[0]
pa=plans[len(pm):]
custom=(root/'custom-icon.html').read_text().replace('__TASKQ_SUMMARY_CONFIG__',json.dumps(json.loads((root/'icon-summary.config.json').read_text()),separators=(',',':')))
cm=custom.split('<style>')[0]
ca=custom[len(cm):]
settings = (root / 'agent-settings.html').read_text()
markup = settings.split('<style>')[0]
assets = settings[len(markup):]
assets = assets.replace("MAC · ${agent}", "${document.querySelector('[data-preview=platform]')?.value || 'macOS'} · ${agent}")
menu = menu.replace('<div class="viz-carousel viz-dotted-background" aria-label="TaskQ review placement">\n<section data-variant="Menu review"><div class="tq-stage" data-layout="menu"></div></section>\n<section data-variant="Workspace review" hidden><div class="tq-stage" data-layout="workspace"></div></section>', '<div aria-label="Workspace review">\n<section><div class="tq-stage" data-layout="workspace"></div></section>')

menu = menu.replace("const root=document.getElementById('tqnext');", "const root=document.getElementById('tqnext');const iconPanel=document.getElementById('tqicon'),iconStash=document.getElementById('icon-stash');const agentPanel=document.getElementById('tqas'),stash=document.getElementById('agent-stash'),aiPanel=document.getElementById('tqai'),aiStash=document.getElementById('ai-stash'),behaviorPanel=document.getElementById('tqbehavior'),behaviorStash=document.getElementById('behavior-stash'),planPanel=document.getElementById('tqplans'),planStash=document.getElementById('plan-stash');")
menu = menu.replace('function render(s){s.el.style', 'function render(s){if(iconPanel.parentNode!==iconStash)iconStash.append(iconPanel);if(agentPanel.parentNode!==stash)stash.append(agentPanel);if(aiPanel.parentNode!==aiStash)aiStash.append(aiPanel);if(behaviorPanel.parentNode!==behaviorStash)behaviorStash.append(behaviorPanel);if(planPanel.parentNode!==planStash)planStash.append(planPanel);s.el.style')
menu = menu.replace("window.lucide?.createIcons();draw(s);}", "if(s.panel==='agents'){const body=s.el.querySelector('.body');body.innerHTML='<div class=\"line\"><button data-action=\"agent-access\">Access</button><button data-action=\"agent-delivery\">Delivery</button></div>';body.append(s.agentTab==='delivery'?behaviorPanel:agentPanel);body.querySelector('[data-action=agent-access]').setAttribute('aria-pressed',s.agentTab!=='delivery');body.querySelector('[data-action=agent-delivery]').setAttribute('aria-pressed',s.agentTab==='delivery');}if(s.panel==='chat'){s.el.querySelector('.body').replaceChildren(aiPanel);}if(s.panel==='plans'){s.el.querySelector('.body').replaceChildren(planPanel);}if(s.panel==='custom-icon'){s.el.querySelector('.body').replaceChildren(iconPanel);window.tqIcon.refresh();}window.lucide?.createIcons();draw(s);}")
menu = menu.replace("${s.panel==='chat'?chat(s)", "${s.panel==='agents'?'':s.panel==='chat'?chat(s)")
menu = menu.replace("${button('sessions','Sessions','terminal')}", "${button('sessions','Sessions','terminal')}${button('agents','Agents','bot')}")
menu = menu.replace("case 'chat':s.panel='chat';", "case 'custom-icon':s.panel='custom-icon';break;case 'plans':s.panel='plans';break;case 'agent-access':s.agentTab='access';break;case 'agent-delivery':s.agentTab='delivery';break;case 'agents':s.panel='agents';break;case 'chat':s.panel='chat';")
for event in ('click','input','change','drop','dragover'):
    menu=menu.replace("root.addEventListener('"+event+"',ev=>{", "root.addEventListener('"+event+"',ev=>{if(ev.target.closest('#tqas,#tqai,#tqbehavior,#tqplans,#tqicon'))return;")
menu=menu.replace("case 'conversations':s.panel='conversations';", "case 'conversations':s.panel='chat';window.tqAI.showList();")
menu=menu.replace("case 'new':s.conversation='New conversation';s.draft='';s.files=[];s.messages=[];s.panel='chat';", "case 'new':window.tqAI.newConversation();s.panel='chat';")
menu=menu.replace("s.messages.push(s.draft);s.draft='';s.files=[];s.panel='chat';feedback('Demo message added · no request sent');", "window.tqAI.acceptMenu(s.draft,s.files);s.draft='';s.files=[];s.panel='chat';feedback('New conversation · simulation only');")
menu=menu.replace('taskq + agentq · Codex</small><span>', 'New conversation · taskq · Codex</small><span>')
menu=menu.replace("accepted.map(f=>({name:f.name,size:f.size}))", "accepted")
# Read image thumbnails locally; selected File objects remain available for handoff.
menu=menu.replace('function addFiles(s,files){', 'async function addFiles(s,files){')
menu=menu.replace("feedback(accepted.length===files.length?'Attachment metadata added · prototype sends no bytes':'Unsupported files rejected; draft preserved');render(s)", "for(const f of accepted){if(f.type.startsWith('image/')&&f.size<=5*1024*1024){try{f.preview=await new Promise((resolve,reject)=>{const reader=new FileReader();reader.onload=()=>resolve(reader.result);reader.onerror=reject;reader.readAsDataURL(f)});}catch{feedback('Preview unavailable; draft preserved');}}}feedback(accepted.length===files.length?'Local attachments selected · no upload':'Unsupported files rejected; draft preserved');render(s)")
menu=menu.replace('<span>${esc(f.name)} ${button', '<span>${f.preview?`<img src="${f.preview}" alt="Attachment preview" style="width:40px;height:40px;object-fit:contain">`:""}${esc(f.name)} ${button')
# Inline decisions reuse one question identity across menu/task routes.
menu=menu.replace("function task(s,n,title,status='good'){return", "function task(s,n,title,status='good'){if(n===841&&status==='ask')return window.tqPlans.question('task');return")
menu=menu.replace("+compose(s,true)+", "+window.tqPlans.summary()+compose(s,true)+")
menu=menu.replace("states.forEach(render);\nmatchMedia", "root.addEventListener('tq-icon-saved',()=>{states[0].view='custom';persist(states[0]);render(states[0])});root.addEventListener('tq-open',ev=>{states[0].panel=ev.detail;render(states[0])});root.addEventListener('tq-refresh',()=>states.forEach(render));states.forEach(render);\nmatchMedia")
# Scope host and type to the composed view; the component owns permission drafts.
menu=menu.replace("s.el.style.setProperty('--type',preview.font+'px');", "s.el.style.setProperty('--type',preview.font+'px');agentPanel.style.setProperty('--size',preview.font+'px');")
# Hide duplicated window chrome, retaining the contextual agent header and scenario control.
css='<style>#tqnext #tqas .window{border:0;border-radius:0}#tqnext #tqas .window>header{display:none}#tqnext #tqas .demo>span,#tqnext #tqas .demo>label:last-child{display:none}#tqnext #tqas main{padding:8px 0}#tqnext #tqas .runtime{padding:0 0 6px}#tqnext #tqas .section{padding-top:8px}#tqnext .tq-stage{min-height:0}#tqnext #tqas footer{padding:7px 0}#tqnext #tqas button.primary{background:var(--ink);color:var(--paper)}</style>'
output='<div id="agent-stash" hidden>'+markup+'</div>\n'+assets+'\n<div id="ai-stash" hidden>'+aimarkup+'</div>\n'+aiassets+'\n<div id="behavior-stash" hidden>'+bmarkup+'</div>\n'+bassets+'\n<div id="plan-stash" hidden>'+pm+'</div>\n'+pa+'\n<div id="icon-stash" hidden>'+cm+'</div>\n'+ca+'\n'+menu+'\n'+css
# Each component saves its own section without erasing another component's drafts.
output=output.replace('window.openai?.setWidgetState?.(', 'window.tqPreviewSave(')
bridge="""<script>window.tqPreviewState=window.openai?.widgetState||{};window.tqPreviewSave=function(next){const previous=window.tqPreviewState;window.tqPreviewState={modelContent:{...previous.modelContent,...next.modelContent},privateContent:{...previous.privateContent,...next.privateContent}};return window.openai?.setWidgetState?.(window.tqPreviewState)};</script>"""
(root/'workspace-prototype.html').write_text(bridge+output)
print('Composed workspace-prototype.html from menu, settings and AI sources')
