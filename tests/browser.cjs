// Playwright launch options for browser instruments (taskq.md § Verification). Default: Playwright's chromium headless
// shell. PLAYWRIGHT_CHANNEL (e.g. `chrome`) picks an installed browser outside a worker only: the Google Chrome app
// aborts at startup under the taskq service (HIServices RegisterApplication) and leaves a crash dialog on the Mac.
// Setup: npm i --no-save playwright && npx playwright install chromium-headless-shell
module.exports = function launchOptions(o = {}) {
  const channel = process.env.PLAYWRIGHT_CHANNEL;
  if (channel && process.env.TASKQ_TASK) {
    console.log(`PLAYWRIGHT_CHANNEL=${channel} ignored in a taskq worker: chromium headless shell`);
    return {...o, headless: true};
  }
  return {...o, headless: true, ...(channel && {channel})};
};
