// Executes the depot UI (index.html + app.js) under jsdom against a LIVE
// server, drives the hash router, and checks each view renders real data.
// This is the browser test for a UI with no build step: the same script tag
// the binary serves, the same fetches, real DOM events.
//
//   depot init --root /tmp/d1 && depot ingest crate ... && depot ingest rpm ...
//   depot ingest toolchain ...            (the checks expect 2 crates, one
//                                          rpm repo "test-repo" with 4 pkgs,
//                                          and 1.96.0 toolchains - the
//                                          testdata fixtures + a pack-rust set)
//   depot serve --root /tmp/d1 --bind 127.0.0.1:8123 --quiet &
//   mkdir /tmp/uitest && cd /tmp/uitest && npm install jsdom@24
//   node /path/to/ui-test.js               -> "31 passed, 0 failed"
//
// Two harness lessons, paid for: (1) do NOT dispatch hashchange by hand -
// jsdom fires it on `location.hash =`, and a manual dispatch renders every
// view twice so clicks land on detached elements; (2) settle on text unique
// to the view you navigated to, not on a wrapper both views share. (3) The
// verify step re-hashes every object on the server: one timeout was seen on
// the first run after a server start with the default 4 s budget; it gets
// 15 s. Three runs after that were clean.
const { JSDOM } = require('jsdom');
const BASE = 'http://127.0.0.1:8123';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  const html = await (await fetch(BASE + '/')).text();
  const dom = new JSDOM(html, {
    url: BASE + '/', runScripts: 'dangerously', resources: 'usable', pretendToBeVisual: true,
    beforeParse(window) {
      window.fetch = (u, o) => fetch(new URL(String(u), BASE).href, o);
      if (!window.CSS) window.CSS = { escape: (s) => String(s).replace(/[^a-zA-Z0-9_-]/g, (c) => '\\' + c) };
      window.alert = (m) => { window.__alert = m; };
    },
  });
  const w = dom.window, d = w.document;
  const settle = async (pred, ms = 4000) => { const t = Date.now(); while (Date.now() - t < ms) { const h = d.querySelector('#main').innerHTML; if (pred(h)) return h; await sleep(50); } throw new Error('timeout; main=' + d.querySelector('#main').innerHTML.slice(0, 200)); };
  const go = async (hash, pred) => { w.location.hash = hash; return settle(pred); };
  let pass = 0, fail = 0;
  const check = (name, ok, extra = '') => { if (ok) { pass++; console.log('ok   ' + name); } else { fail++; console.log('FAIL ' + name + ' ' + extra); } };

  let h = await settle((h) => h.includes('<h1>Overview</h1>'));
  check('overview renders counts', /crates · 2 versions/.test(h) && /RPMs · 1 repos/.test(h) && /toolchain files/.test(h));
  check('overview shows signing key + writes disabled', /signing key/.test(h) && /disabled/.test(h));

  h = await go('#crates', (h) => h.includes('<h1>Crates'));
  check('crates table lists both crates with links', h.includes('#crate/cfg-if') && h.includes('#crate/unicode-ident'));

  h = await go('#crate/cfg-if', (h) => h.includes('<h1>cfg-if</h1>'));
  check('crate view shows version, size, index link, yank button', /1\.0\.0/.test(h) && /7\.7 KiB/.test(h) && h.includes('/crates/index/cf/g-/cfg-if') && h.includes('data-yank'));
  d.querySelector('[data-yank]').click(); await sleep(100);
  check('yank without a token alerts instead of firing', /admin token/.test(w.__alert || ''), w.__alert);

  h = await go('#rpm', (h) => h.includes('RPM repositories'));
  check('rpm repo list: test-repo, 4 packages, no key warning', h.includes('#rpm/test-repo') && /<td class="num">4<\/td>/.test(h) && /gpgcheck=0/.test(h));
  h = await go('#rpm/test-repo', (h) => h.includes('<h1>test-repo'));
  check('rpm package list with nevra + hrefs', h.includes('edge-3:1.2.3-4.fc42.noarch') && h.includes('/rpm/test-repo/Packages/h/hello-2.0-1.x86_64.rpm'));

  h = await go('#toolchains', (h) => h.includes('Rust toolchains'));
  check('toolchains lists the three 1.96.0 components + install one-liner', /rustc/.test(h) && /rust-std/.test(h) && /cargo/.test(h) && h.includes('toolchains/install.sh'));

  h = await go('#nix', (h) => h.includes('Nix binary cache'));
  check('nix view (empty) shows the how-to', h.includes('nix copy --to file://'));

  h = await go('#sync', (h) => h.includes('<h1>Sync</h1>'));
  check('sync view embeds this depot signing key in the trust command', /depot trust online [0-9a-f]{64}/.test(h));
  d.querySelector('#tok').value = 'abc'; d.querySelector('#savetok').click();
  check('token saved to sessionStorage', w.sessionStorage.getItem('depot-token') === 'abc');
  d.querySelector('#verify').click();
  const v = await settle((h) => /every object hashes|corrupt/.test(h), 15000); // hashes every object; depends on the server's disk
  check('verify button calls /api/verify and reports', /every object hashes to its name/.test(v));

  h = await go('#clients', (h) => h.includes('Client configuration'));
  await settle((h) => h.includes('replace-with'));
  h = d.querySelector('#main').innerHTML;
  check('clients: cargo + dnf + toolchain snippets with the live URL', h.includes('sparse+http://127.0.0.1:8123/crates/index/') && h.includes('/etc/yum.repos.d/depot-test-repo.repo') && h.includes('install.sh'));
  d.querySelector('#w-nix').click(); await settle((h) => h.includes('substituters ='));
  check('ticking nix refreshes snippets', true);
  check('bootstrap one-liner reflects the ticks', d.querySelector('#boot').textContent.includes('nix=1') && d.querySelector('#boot').textContent.includes('rpm=test-repo'));

  h = await go('#manual', (h) => h.includes('<div class="md">'));
  check('manual renders headings/tables/code', /<h2 id="[^"]*">/.test(h) && h.includes('<table>') && h.includes('<pre><code>') && !h.includes('```'));
  h = await go('#decisions', (h) => h.includes('Data root:'));
  check('decisions renders 15 numbered sections', (h.match(/<h2 /g) || []).length >= 15);
  h = await go('#tutorials', (h) => h.includes('<h1>Tutorials</h1>'));
  check('tutorials list shows six with step counts', (h.match(/class="card"/g) || []).length === 6 && /10 steps/.test(h) && h.includes('#tutorial/usb-link'));
  h = await go('#tutorial/sync', (h) => h.includes('Step 1 of 7'));
  check('stepper opens at step 1 with intro, toc, body, copy buttons', h.includes('How a pack works') && (h.match(/steps-toc/g) || []).length === 1 && d.querySelectorAll('.step-body .copy').length >= 1 && !h.includes('previous'));
  d.querySelector('#markdone').click();
  h = await settle((h) => /1 done/.test(h));
  check('mark done updates progress and persists', /done ✓/.test(h) && JSON.parse(w.localStorage.getItem('depot-tut-sync')).done.includes(0));
  await settle((h) => h.includes('<svg ') && h.includes('One pack, two depots'), 6000);
  check('niftyplot fence in a step renders to inline SVG via /api/niftyplot, source kept', d.querySelectorAll('.niftyplot-fig svg').length === 1 && d.querySelector('.niftyplot-fig details pre').textContent.includes('%kind: graph'));
  check('read-aloud button hidden when speechSynthesis is absent', d.querySelector('#say').hidden === true);
  h = await go('#tutorial/fill-the-depot/1', (h) => h.includes('Step 2 of 8'));
  check('a satisfied check reads as already done, with evidence', /class="check ok"/.test(h) && /2 crates, 2 versions/.test(h));
  h = await go('#tutorial/sync/1', (h) => h.includes('Step 2 of 7'));
  check('step with a %check shows a live badge from this depot (trust/ is empty here)', /class="check todo"/.test(h) && /Not yet on this depot: trust\/ is empty/.test(h));
  check('next step: previous button appears, intro hidden, toc marks step 1 done', h.includes('previous') && !h.includes('Two depots: one that can see') && /class="[^"]*done[^"]*"/.test(h));
  h = await go('#tutorial/sync/6', (h) => h.includes('Step 7 of 7'));
  check('last step offers finish', h.includes('finish') && !h.includes('next →'));
  h = await go('#tutorials', (h) => h.includes('<h1>Tutorials</h1>'));
  check('list shows progress and a continue link at the remembered step', /1 done/.test(h) && h.includes('#tutorial/sync/6'));
  h = await go('#tutorial/does-not-exist', (h) => /no such tutorial/.test(h));
  check('unknown tutorial reports', true);
  check('/api/tutorials/sync.md is the raw file', (await (await fetch(BASE + '/api/tutorials/sync.md')).text()).startsWith('# Move content'));
  // Path-based entry: /tutorials with no hash lands on the tutorials view.
  {
    const dom2 = new JSDOM(await (await fetch(BASE + '/tutorials')).text(), { url: BASE + '/tutorials', runScripts: 'dangerously', resources: 'usable', pretendToBeVisual: true,
      beforeParse(win) { win.fetch = (u, o) => fetch(new URL(String(u), BASE).href, o); win.CSS = { escape: (x) => x }; } });
    const t0 = Date.now(); let ok = false;
    while (Date.now() - t0 < 4000) { if (dom2.window.document.querySelector('#main').innerHTML.includes('<h1>Tutorials</h1>')) { ok = true; break; } await sleep(50); }
    check('/tutorials as a plain URL opens the tutorials view', ok);
  }
  h = await go('#nope', (h) => /no such view/.test(h));
  check('unknown route reports', true);
  console.log(`\n${pass} passed, ${fail} failed`);
  process.exit(fail ? 1 : 0);
})().catch((e) => { console.error('ERROR', e); process.exit(2); });
