/* depot UI — plain DOM + fetch. Views are functions of /api responses; the
   hash is the route. Nothing here mutates the depot except the explicit
   "yank" button, which needs the admin token typed in. */
(() => {
  const $ = (s, el = document) => el.querySelector(s);
  const main = $('#main');
  const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const human = (n) => { const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB']; let i = 0; n = Number(n) || 0; while (n >= 1024 && i < 4) { n /= 1024; i++; } return i ? n.toFixed(1) + ' ' + u[i] : n + ' B'; };
  const when = (t) => t ? new Date(t * 1000).toISOString().replace('T', ' ').slice(0, 16) + ' UTC' : '—';
  const api = async (p, opt) => { const r = await fetch(p, opt); if (!r.ok) { let m = r.statusText; try { m = (await r.json()).error || m; } catch (_) { try { m = await r.text(); } catch (__) {} } throw new Error(r.status + ' ' + m); } return r.headers.get('content-type')?.includes('json') ? r.json() : r.text(); };
  const base = () => location.origin;
  const token = () => sessionStorage.getItem('depot-token') || '';

  // ── tiny markdown (enough for MANUAL.md / DECISIONS.md) ─────────────────────
  function md(src) {
    const lines = src.split('\n'); let out = ''; let i = 0;
    const inline = (s) => esc(s)
      .replace(/`([^`]+)`/g, '<code>$1</code>')
      .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
      .replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>')
      .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>');
    while (i < lines.length) {
      const l = lines[i];
      if (/^```/.test(l)) { let b = ''; i++; while (i < lines.length && !/^```/.test(lines[i])) { b += lines[i] + '\n'; i++; } i++; out += '<pre><code>' + esc(b) + '</code></pre>'; continue; }
      const h = /^(#{1,6})\s+(.*)/.exec(l);
      if (h) { const id = h[2].toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, ''); out += `<h${h[1].length} id="${id}">${inline(h[2])}</h${h[1].length}>`; i++; continue; }
      if (/^\s*[-*]\s+/.test(l)) { out += '<ul>'; while (i < lines.length && /^\s*[-*]\s+/.test(lines[i])) { out += '<li>' + inline(lines[i].replace(/^\s*[-*]\s+/, '')) + '</li>'; i++; } out += '</ul>'; continue; }
      if (/^\s*\d+\.\s+/.test(l)) { out += '<ol>'; while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) { out += '<li>' + inline(lines[i].replace(/^\s*\d+\.\s+/, '')) + '</li>'; i++; } out += '</ol>'; continue; }
      if (/^\|/.test(l)) { const rows = []; while (i < lines.length && /^\|/.test(lines[i])) { rows.push(lines[i]); i++; } const cells = (r) => r.replace(/^\||\|$/g, '').split('|').map((c) => inline(c.trim())); out += '<table><thead><tr>' + cells(rows[0]).map((c) => '<th>' + c + '</th>').join('') + '</tr></thead><tbody>' + rows.slice(2).map((r) => '<tr>' + cells(r).map((c) => '<td>' + c + '</td>').join('') + '</tr>').join('') + '</tbody></table>'; continue; }
      if (/^>\s?/.test(l)) { let b = ''; while (i < lines.length && /^>\s?/.test(lines[i])) { b += lines[i].replace(/^>\s?/, '') + ' '; i++; } out += '<blockquote>' + inline(b) + '</blockquote>'; continue; }
      if (!l.trim()) { i++; continue; }
      let p = ''; while (i < lines.length && lines[i].trim() && !/^(#|```|\||>|\s*[-*]\s|\s*\d+\.\s)/.test(lines[i])) { p += lines[i] + ' '; i++; }
      out += '<p>' + inline(p.trim()) + '</p>';
    }
    return out;
  }

  // ── views ───────────────────────────────────────────────────────────────────
  const views = {
    async overview() {
      const s = await api('/api/status');
      const c = s.counts;
      return `<h1>Overview</h1>
      <div class="cards">
        <div class="card"><div class="n">${c.crates}</div><div class="l">crates · ${c.crate_versions} versions</div></div>
        <div class="card"><div class="n">${c.rpm_packages}</div><div class="l">RPMs · ${c.rpm_repos} repos</div></div>
        <div class="card"><div class="n">${c.nix_paths}</div><div class="l">nix store paths</div></div>
        <div class="card"><div class="n">${c.toolchain_files}</div><div class="l">toolchain files</div></div>
        <div class="card"><div class="n">${human(s.bytes)}</div><div class="l">${s.objects} objects on disk</div></div>
      </div>
      <table>
        <tr><th>root</th><td><code>${esc(s.root)}</code></td></tr>
        <tr><th>bind</th><td><code>${esc(s.bind)}</code></td></tr>
        <tr><th>last change</th><td>${when(s.updated)}</td></tr>
        <tr><th>signing key</th><td><code>${esc(s.signing_key || '(none)')}</code> <span class="muted">— give this to depots that will unpack from here</span></td></tr>
        <tr><th>trusts</th><td>${s.trusted.length ? s.trusted.map((t) => `<span class="pill">${esc(t)}</span>`).join(' ') : '<span class="muted">nobody yet — <code>depot trust NAME HEX</code></span>'}</td></tr>
        <tr><th>HTTP writes</th><td>${s.writes_enabled ? '<span class="ok">enabled</span> (admin token present)' : '<span class="muted">disabled</span> — read-only until <code>depot admin-token --new</code>'}</td></tr>
      </table>
      <div class="note">A device points at <code>${esc(base())}</code>. Open <a href="#clients">Clients</a> for the exact files to write, or run <code>curl -O ${esc(base())}/bootstrap.sh</code> on the device.</div>`;
    },

    async crates(q) {
      const d = await api('/api/crates?q=' + encodeURIComponent(q || ''));
      return `<h1>Crates <span class="muted">${d.total}</span></h1>
      <div class="row"><input type="search" id="q" placeholder="filter by name" value="${esc(q || '')}"><span class="muted">sparse index at <code>${esc(base())}/crates/index/</code></span></div>
      <table><thead><tr><th>name</th><th class="num">versions</th><th>latest</th><th>yanked</th></tr></thead><tbody>
      ${d.crates.map((c) => `<tr><td><a href="#crate/${esc(c.name)}">${esc(c.name)}</a></td><td class="num">${c.versions}</td><td>${esc(c.latest)}</td><td>${c.yanked ? c.yanked : ''}</td></tr>`).join('')}
      </tbody></table>${d.crates.length >= 500 ? '<p class="muted">showing the first 500; narrow the filter.</p>' : ''}`;
    },

    async crate(name) {
      const d = await api('/api/crates/' + encodeURIComponent(name));
      return `<h1>${esc(d.name)}</h1>
      <p>index file: <a href="${esc(d.index)}"><code>${esc(d.index)}</code></a></p>
      <table><thead><tr><th>version</th><th class="num">size</th><th class="num">deps</th><th>sha256</th><th>yanked</th><th></th></tr></thead><tbody>
      ${d.versions.map((v) => `<tr><td><a href="${esc(v.path)}">${esc(v.vers)}</a></td><td class="num">${human(v.size)}</td><td class="num">${v.deps}</td><td><code>${esc(v.hash.slice(0, 16))}…</code></td><td>${v.yanked ? '<span class="bad">yes</span>' : ''}</td><td><button class="secondary" data-yank="${esc(d.name)}|${esc(v.vers)}|${v.yanked ? 'unyank' : 'yank'}">${v.yanked ? 'unyank' : 'yank'}</button></td></tr>`).join('')}
      </tbody></table>
      <p class="muted">Yanking needs the admin token (Sync tab) and marks the version <code>"yanked":true</code> in the index; the file stays downloadable, as on crates.io.</p>`;
    },

    async rpm(id, q) {
      if (!id) {
        const d = await api('/api/rpm');
        return `<h1>RPM repositories</h1>
        <table><thead><tr><th>repo</th><th>description</th><th class="num">packages</th><th class="num">size</th><th>GPG key</th></tr></thead><tbody>
        ${d.repos.map((r) => `<tr><td><a href="#rpm/${esc(r.id)}">${esc(r.id)}</a></td><td>${esc(r.description)}</td><td class="num">${r.packages}</td><td class="num">${human(r.bytes)}</td><td>${r.gpg_key ? '<span class="ok">attached</span>' : '<span class="bad">none</span> — clients get gpgcheck=0'}</td></tr>`).join('') || '<tr><td colspan="5" class="muted">no repos yet — <code>depot ingest rpm --repo fedora-42-x86_64 *.rpm</code></td></tr>'}
        </tbody></table>
        <div class="note">Each repo is a complete dnf repository at <code>${esc(base())}/rpm/&lt;repo&gt;/</code> with its own <code>repodata/</code>. Repo ids are yours: name them by distribution, release and arch.</div>`;
      }
      const d = await api('/api/rpm/' + encodeURIComponent(id) + '?q=' + encodeURIComponent(q || ''));
      return `<h1>${esc(d.id)} <span class="muted">${d.total}</span></h1>
      <p>${esc(d.description)} · <a href="/rpm/${esc(d.id)}/repodata/repomd.xml"><code>repodata/repomd.xml</code></a> · GPG key ${d.gpg_key ? `<a href="/rpm/${esc(d.id)}/RPM-GPG-KEY">attached</a>` : '<span class="bad">none</span>'}</p>
      <div class="row"><input type="search" id="q" placeholder="filter by package name" value="${esc(q || '')}"></div>
      <table><thead><tr><th>package</th><th>arch</th><th class="num">size</th><th>sha256</th></tr></thead><tbody>
      ${d.packages.map((p) => `<tr><td><a href="${esc(p.path)}">${esc(p.nevra)}</a></td><td>${esc(p.arch)}</td><td class="num">${human(p.size)}</td><td><code>${esc(p.hash.slice(0, 16))}…</code></td></tr>`).join('')}
      </tbody></table>`;
    },

    async nix() {
      const d = await api('/api/nix');
      return `<h1>Nix binary cache <span class="muted">${d.total}</span></h1>
      <p>substituter URL: <code>${esc(base())}/nix</code> · <a href="/nix/nix-cache-info"><code>nix-cache-info</code></a></p>
      <table><thead><tr><th>store path</th><th class="num">NAR</th><th></th></tr></thead><tbody>
      ${d.paths.map((p) => `<tr><td><code>${esc(p.store_path)}</code></td><td class="num">${human(p.nar_size)}</td><td><a href="${esc(p.narinfo)}">narinfo</a> · <a href="${esc(p.nar)}">nar</a></td></tr>`).join('') || '<tr><td colspan="3" class="muted">empty — on a machine with nix: <code>nix copy --to file:///tmp/cache nixpkgs#hello</code>, then <code>depot ingest nix /tmp/cache</code></td></tr>'}
      </tbody></table>`;
    },

    async toolchains() {
      const d = await api('/api/toolchains');
      return `<h1>Rust toolchains</h1>
      <p>installer: <a href="/toolchains/install.sh"><code>${esc(base())}/toolchains/install.sh</code></a> · <a href="/toolchains/index.json"><code>index.json</code></a></p>
      <table><thead><tr><th>version</th><th>triple</th><th>component</th><th class="num">size</th></tr></thead><tbody>
      ${d.toolchains.map((t) => `<tr><td>${esc(t.version)}</td><td>${esc(t.triple)}</td><td><a href="${esc(t.path)}">${esc(t.component)}</a></td><td class="num">${human(t.size)}</td></tr>`).join('') || '<tr><td colspan="4" class="muted">empty — <code>depot ingest toolchain rust-install/</code> (from pack-rust.sh)</td></tr>'}
      </tbody></table>
      <pre><code>curl -fsSL ${esc(base())}/toolchains/install.sh -o i.sh &amp;&amp; sh i.sh ${esc(d.toolchains[0]?.version || '<version>')} "$HOME/.local"</code></pre>`;
    },

    async sync() {
      const s = await api('/api/status');
      return `<h1>Sync</h1>
      <h2>Sneakernet (no network between the two depots)</h2>
      <pre><code># on the OFFLINE depot: what do I already have?
depot have --out have.txt            # carry have.txt to the online side

# on the ONLINE depot: everything the offline one lacks, signed
depot pack --out sync.depot --have have.txt --all

# back on the OFFLINE depot (once): trust the online depot's key
depot trust online ${esc(s.signing_key || '<hex from the other side>')}
depot unpack sync.depot</code></pre>
      <h2>Over HTTP (the offline depot can reach this one)</h2>
      <pre><code># pull a pack of what THIS depot lacks from ${esc(base())} (run on the other depot):
depot have | curl -sS --data-binary @- "${esc(base())}/api/pack?select=all" -o sync.depot
depot unpack sync.depot

# or push a pack INTO this depot (needs its admin token):
curl -sS -X PUT -H "Authorization: Bearer $TOKEN" --data-binary @sync.depot ${esc(base())}/api/packs</code></pre>
      <h2>Direct uploads (needs the admin token)</h2>
      <pre><code>curl -X PUT -H "Authorization: Bearer $TOKEN" --data-binary @foo-1.2.3.crate ${esc(base())}/api/crates
curl -X PUT -H "Authorization: Bearer $TOKEN" --data-binary @foo.rpm         ${esc(base())}/api/rpm/fedora-42-x86_64
curl -X PUT -H "Authorization: Bearer $TOKEN" -H "X-Filename: rustc-1.96.0-aarch64-unknown-linux-musl.tar.xz" \\
     --data-binary @rustc-1.96.0-aarch64-unknown-linux-musl.tar.xz ${esc(base())}/api/toolchains</code></pre>
      <h2>Admin token for this browser session</h2>
      <div class="row"><input type="text" id="tok" placeholder="paste the output of: depot admin-token" value="${esc(token())}" size="48"><button id="savetok">use</button><span class="muted">kept in sessionStorage only; enables yank buttons here</span></div>
      <h2>Integrity</h2>
      <div class="row"><button class="secondary" id="verify">re-hash every object</button><span id="verifyout" class="muted"></span></div>`;
    },

    async clients() {
      const rpm = await api('/api/rpm');
      const repos = rpm.repos.map((r) => r.id);
      const sel = () => { const q = new URLSearchParams(); q.set('crates', $('#w-crates')?.checked ? 1 : 0); q.set('nix', $('#w-nix')?.checked ? 1 : 0); q.set('toolchain', $('#w-tc')?.checked ? 1 : 0); repos.forEach((r) => { if ($('#w-rpm-' + CSS.escape(r))?.checked) q.append('rpm', r); }); return q.toString(); };
      setTimeout(async () => {
        const render = async () => {
          const d = await api('/api/client-config?' + sel());
          $('#snips').innerHTML = d.snippets.map((s) => `<h3>${esc(s.kind)} → <code>${esc(s.path)}</code></h3><pre><code>${esc(s.contents)}</code></pre><p class="muted">then: <code>${esc(s.after)}</code></p>`).join('') || '<p class="muted">tick something above.</p>';
          $('#boot').textContent = `curl -fsSL "${base()}/bootstrap.sh?${sel()}" -o bootstrap.sh && sh -c 'DRY=1 sh bootstrap.sh' && sudo sh bootstrap.sh`;
        };
        main.querySelectorAll('input[type=checkbox]').forEach((c) => c.addEventListener('change', render));
        render();
      }, 0);
      return `<h1>Client configuration</h1>
      <p>Tick what this device should use; the files appear below with the URL <code>${esc(base())}</code> baked in.</p>
      <div class="row">
        <label><input type="checkbox" id="w-crates" checked> cargo crates</label>
        ${repos.map((r) => `<label><input type="checkbox" id="w-rpm-${esc(r)}" checked> dnf: ${esc(r)}</label>`).join('')}
        <label><input type="checkbox" id="w-nix"> nix</label>
        <label><input type="checkbox" id="w-tc" checked> rust toolchain</label>
      </div>
      <h2>One-liner</h2><pre><code id="boot"></code></pre>
      <p class="muted">The script prints every file before writing it; <code>DRY=1</code> only prints. It is generated, not stored — read it.</p>
      <div id="snips"></div>`;
    },

    async manual() { return '<div class="md">' + md(await api('/api/manual.md')) + '</div>'; },
    async decisions() { return '<div class="md">' + md(await api('/api/decisions.md')) + '</div>'; },
  };

  // ── router ─────────────────────────────────────────────────────────────────
  async function route() {
    const h = location.hash.slice(1) || 'overview';
    const [view, ...rest] = h.split('/');
    const arg = rest.map(decodeURIComponent);
    document.querySelectorAll('#nav a').forEach((a) => a.classList.toggle('active', a.getAttribute('href') === '#' + view || (view === 'crate' && a.getAttribute('href') === '#crates')));
    try {
      const q = new URLSearchParams(location.search).get('q') || '';
      let html;
      if (view === 'crate') html = await views.crate(arg[0]);
      else if (view === 'rpm') html = await views.rpm(arg[0], q);
      else if (views[view]) html = await views[view](q);
      else html = '<p class="bad">no such view</p>';
      main.innerHTML = html;
      wire(view, arg);
      if (rest.length && (view === 'manual' || view === 'decisions')) document.getElementById(arg[0])?.scrollIntoView();
    } catch (e) {
      main.innerHTML = `<p class="bad">${esc(e.message)}</p>`;
    }
  }

  function wire(view) {
    const q = $('#q');
    if (q) q.addEventListener('keydown', (e) => { if (e.key === 'Enter') { const u = new URL(location.href); u.searchParams.set('q', q.value); history.replaceState(null, '', u); route(); } });
    main.querySelectorAll('[data-yank]').forEach((b) => b.addEventListener('click', async () => {
      const [name, vers, action] = b.dataset.yank.split('|');
      if (!token()) { alert('Set the admin token on the Sync tab first.'); return; }
      try { await api(`/api/crates/${encodeURIComponent(name)}/${encodeURIComponent(vers)}/${action}`, { method: 'POST', headers: { Authorization: 'Bearer ' + token() } }); route(); } catch (e) { alert(e.message); }
    }));
    $('#savetok')?.addEventListener('click', () => { sessionStorage.setItem('depot-token', $('#tok').value.trim()); $('#savetok').textContent = 'saved'; });
    $('#verify')?.addEventListener('click', async () => { $('#verifyout').textContent = 'hashing…'; try { const r = await api('/api/verify'); $('#verifyout').innerHTML = r.corrupt.length ? `<span class="bad">${r.corrupt.length} corrupt: ${esc(r.corrupt.join(', '))}</span>` : '<span class="ok">every object hashes to its name</span>'; } catch (e) { $('#verifyout').textContent = e.message; } });
  }

  window.addEventListener('hashchange', route);
  api('/api/status').then((s) => { $('#foot').textContent = `${s.root} · ${s.objects} objects · ${human(s.bytes)}`; }).catch(() => {});
  route();
})();
