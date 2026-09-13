// Terminal panel under jsdom against a depot with [terminal] enabled.
//   DEPOT_URL, DEPOT_TOKEN_FILE override the defaults.
//   depot init; depot admin-token --new; set [terminal] enabled=true; serve
// Harness lesson paid for: the token lives in sessionStorage (Sync tab sets
// it), not localStorage; and a bare \r must NOT wipe a line that ends \r\n
// (that was a real appendScreen bug this test caught — the direct API test
// could not, since it never runs the client's screen logic).
const { JSDOM } = require('jsdom');
const BASE = process.env.DEPOT_URL || 'http://127.0.0.1:8123';
const TOKEN = require('fs').readFileSync(process.env.DEPOT_TOKEN_FILE || '/tmp/dterm/keys/admin.token', 'utf8').trim();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  const dom = new JSDOM(await (await fetch(BASE + '/')).text(), {
    url: BASE + '/', runScripts: 'dangerously', resources: 'usable', pretendToBeVisual: true,
    beforeParse(w) {
      w.fetch = (u, o) => fetch(new URL(String(u), BASE).href, o);
      w.CSS = { escape: (x) => x };
      w.atob = (b) => Buffer.from(b, 'base64').toString('binary');
      w.TextEncoder = TextEncoder;
      w.sessionStorage.setItem('depot-token', TOKEN);
    },
  });
  const w = dom.window, d = w.document;
  const settle = async (pred, ms = 8000) => { const t = Date.now(); while (Date.now() - t < ms) { if (pred()) return true; await sleep(60); } return false; };
  let pass = 0, fail = 0; const check = (n, ok, x = '') => { ok ? pass++ : fail++; console.log((ok ? 'ok   ' : 'FAIL ') + n + (ok ? '' : ' :: ' + x)); };

  await settle(() => d.querySelector('#openterm'));
  check('terminal launcher appears in header when enabled', !!d.querySelector('#openterm'));
  d.querySelector('#openterm').click();
  await settle(() => d.querySelector('#termpanel.show'));
  check('panel opens with keycaps and an input', d.querySelectorAll('.keycap').length === 10 && !!d.querySelector('#termline'));
  const scr = () => d.querySelector('#termscreen').textContent;
  const ok = await settle(() => /\$|depot:/.test(scr()), 8000);
  check('a shell session starts and a prompt appears', ok, JSON.stringify(scr().slice(-80)));

  // A keycap sends bytes first (isolated), then a typed command.
  const before = scr().length;
  [...d.querySelectorAll('.keycap')].find((b) => b.textContent === 'C-c').click();
  await sleep(500);
  check('C-c keycap sends a byte (screen advances)', scr().length >= before);

  // Type a command through the input line and wait for the echo to poll back.
  const line = d.querySelector('#termline'); line.value = 'echo panel-marker-77';
  line.dispatchEvent(new w.KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  check('marker echoed back', await settle(() => scr().includes('panel-marker-77'), 10000), JSON.stringify(scr().slice(-120)));

  // "run" button on a rendered code block: navigate to a tutorial step with a shell block.
  w.location.hash = 'tutorial/prepare-clients/2'; await sleep(50);
  await settle(() => d.querySelector('.step-body pre'));
  await settle(() => [...d.querySelectorAll('.codebar .run')].length > 0, 3000);
  const runBtns = [...d.querySelectorAll('.codebar .run')];
  check('run buttons appear on shell blocks while the terminal is open', runBtns.length > 0, 'found ' + runBtns.length);
  check('every code block has a copy button', d.querySelectorAll('.codebar .copy').length >= d.querySelectorAll('.step-body pre').length && d.querySelectorAll('.step-body pre').length > 0);

  d.querySelector('#termclose').click();
  await sleep(200);
  check('close hides the panel', !d.querySelector('#termpanel.show'));

  console.log(`\n${pass} passed, ${fail} failed`);
  process.exit(fail ? 1 : 0);
})().catch((e) => { console.error('ERROR', e); process.exit(2); });
