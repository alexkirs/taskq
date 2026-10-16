// Preserve the public component APIs exposed by the original tray page.
globalThis.TaskqAnswerCard=require('./answer-card.js');
globalThis.TaskqProjectBlocks=require('./project-blocks.js');
globalThis.TaskqResources=require('./resource-controls.js');
const tray=require('./tray-app.js');
tray.create(tray.createState(),{}).mount();
