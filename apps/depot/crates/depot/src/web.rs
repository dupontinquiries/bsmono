//! HTTP routes. Two kinds:
//!
//! * **Artifact paths** (`/crates/…`, `/rpm/…`, `/nix/…`, `/toolchains/…`)
//!   are served straight from `pub/` by `depot_httpd::serve_file` — no lock,
//!   no catalog lookup, no allocation per byte. This is the hot path (a dnf
//!   transaction is hundreds of GETs) and it is deliberately dumb.
//! * **`/api/…`** reads the catalog under a short mutex hold and answers JSON;
//!   writes stream the body to `tmp/`, then take the lock to ingest.
//!
//! Three paths are generated per request because they embed the depot's own
//! URL, which depends on how the client reached us: `/crates/config.json`,
//! `/toolchains/install.sh`, `/bootstrap.sh`.

use crate::checks;
use crate::clientcfg::{self, Want};
use crate::tutorial;
use depot_core::pack::Select;
use depot_core::Depot;
use depot_httpd::{dir_listing, safe_join, serve_file, Body, Request, Response};
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const INDEX_HTML: &str = include_str!("../ui/index.html");
pub const APP_JS: &str = include_str!("../ui/app.js");
pub const STYLE_CSS: &str = include_str!("../ui/style.css");
pub const MANUAL_MD: &str = include_str!("../../../MANUAL.md");
pub const DECISIONS_MD: &str = include_str!("../../../DECISIONS.md");

pub struct App {
    pub depot: Arc<Mutex<Depot>>,
    pub pub_dir: PathBuf,
    pub root: PathBuf,
    pub public_url: String,
    pub token_file: PathBuf,
}

impl App {
    fn base_url(&self, req: &Request<'_>) -> String {
        if !self.public_url.is_empty() {
            return self.public_url.trim_end_matches('/').to_string();
        }
        let host = req.header("host").unwrap_or("localhost:8000");
        format!("http://{host}")
    }

    /// Write endpoints: refused unless the admin token file exists AND the
    /// request carries its contents. The file is re-read per request so a
    /// rotated token takes effect without a restart — the cost is one small
    /// read on a path nobody hits in a loop.
    fn authorize(&self, req: &Request<'_>) -> Result<(), Response> {
        let want = match std::fs::read_to_string(&self.token_file) {
            Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => return Err(Response::text(403, "writes are disabled: no admin token on this depot (depot admin-token --new)\n")),
        };
        let keys = net_kit::auth::Keyring::new(vec![want]);
        match req.token() {
            Some(t) if keys.accepts(t.as_bytes()) => Ok(()),
            _ => Err(Response::text(401, "unauthorized: send Authorization: Bearer <admin token>\n").with_header("WWW-Authenticate", "Bearer")),
        }
    }

    /// Stream a request body straight into the object store — hashed once,
    /// on the way in, WITHOUT the catalog lock. The caller then ingests the
    /// object under the lock, which for an RPM is a header read and for a
    /// crate a tar scan; never a second pass over the bytes. If ingest
    /// rejects it and the object was new, the caller removes it.
    fn receive(&self, req: &mut Request<'_>) -> Result<depot_core::store::Stored, Response> {
        if req.content_length == 0 {
            return Err(Response::text(411, "Content-Length required\n"));
        }
        let store = depot_core::Store::open(&self.root);
        let stored = store.put_reader(&mut req.body, None).map_err(|e| Response::text(400, format!("read body: {e}\n")))?;
        if stored.size != req.content_length {
            if stored.fresh {
                let _ = store.remove(&stored.hash);
            }
            return Err(Response::text(400, format!("short body: {} of {} bytes\n", stored.size, req.content_length)));
        }
        Ok(stored)
    }

    fn discard_if_fresh(&self, stored: &depot_core::store::Stored) {
        if stored.fresh {
            let _ = depot_core::Store::open(&self.root).remove(&stored.hash);
        }
    }

    /// Stream a request body to a temp file under `<root>/tmp/` (packs,
    /// which are not objects themselves).
    fn spool(&self, req: &mut Request<'_>, suffix: &str) -> Result<PathBuf, Response> {
        if req.content_length == 0 {
            return Err(Response::text(411, "Content-Length required\n"));
        }
        let dir = self.root.join("tmp");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(format!("upload-{}-{}{}", std::process::id(), depot_core::now(), suffix));
        let mut f = std::fs::File::create(&p).map_err(|e| Response::text(500, format!("spool: {e}\n")))?;
        match std::io::copy(&mut req.body, &mut f) {
            Ok(n) if n == req.content_length => Ok(p),
            Ok(n) => {
                let _ = std::fs::remove_file(&p);
                Err(Response::text(400, format!("short body: {n} of {} bytes\n", req.content_length)))
            }
            Err(e) => {
                let _ = std::fs::remove_file(&p);
                Err(Response::text(400, format!("read body: {e}\n")))
            }
        }
    }

    fn handle_inner(&self, req: &mut Request<'_>) -> Response {
        let path = req.path.clone();
        let method = req.method.clone();
        let method = method.as_str();

        // ── UI ───────────────────────────────────────────────────────────────
        match (method, path.as_str()) {
            ("GET" | "HEAD", "/") | ("GET" | "HEAD", "/manual") | ("GET" | "HEAD", "/decisions") | ("GET" | "HEAD", "/tutorials") => return Response::html(INDEX_HTML).with_header("Cache-Control", "no-cache"),
            ("GET" | "HEAD", "/app.js") => return Response::bytes(200, "application/javascript; charset=utf-8", APP_JS.as_bytes().to_vec()),
            ("GET" | "HEAD", "/style.css") => return Response::bytes(200, "text/css; charset=utf-8", STYLE_CSS.as_bytes().to_vec()),
            ("GET" | "HEAD", "/api/manual.md") => return Response::bytes(200, "text/markdown; charset=utf-8", MANUAL_MD.as_bytes().to_vec()),
            ("GET" | "HEAD", "/api/decisions.md") => return Response::bytes(200, "text/markdown; charset=utf-8", DECISIONS_MD.as_bytes().to_vec()),
            _ => {}
        }

        // ── generated files ──────────────────────────────────────────────────
        // The sparse protocol: config.json lives at the INDEX root, next to
        // the 1/ 2/ 3/ ab/ directories — cargo asks for <index-url>/config.json.
        if path == "/crates/index/config.json" {
            return Response::json(200, depot_core::crates::render_config(&self.base_url(req)));
        }
        if path == "/toolchains/install.sh" {
            let s = depot_core::toolchain::INSTALL_SH.replace("__DEPOT_URL__", &self.base_url(req));
            return Response::bytes(200, "text/x-shellscript; charset=utf-8", s.into_bytes());
        }
        if path == "/bootstrap.sh" {
            let want = want_from_query(req);
            let d = self.depot.lock().unwrap();
            let s = clientcfg::bootstrap_script(&d, &self.base_url(req), &want);
            return Response::bytes(200, "text/x-shellscript; charset=utf-8", s.into_bytes());
        }

        // ── API ──────────────────────────────────────────────────────────────
        if let Some(rest) = path.strip_prefix("/api/") {
            return self.api(method, rest, req);
        }

        // ── artifacts: static from pub/ ──────────────────────────────────────
        if method != "GET" && method != "HEAD" {
            return Response::text(405, "method not allowed\n");
        }
        let top_ok = ["crates/", "rpm/", "nix/", "toolchains/"].iter().any(|p| path.trim_start_matches('/').starts_with(p));
        if !top_ok {
            return Response::not_found();
        }
        let Some(fs_path) = safe_join(&self.pub_dir, &path) else {
            return Response::text(403, "forbidden\n");
        };
        if fs_path.is_dir() {
            if !path.ends_with('/') {
                return Response::new(301).with_header("Location", format!("{path}/"));
            }
            return dir_listing(&path, &fs_path);
        }
        // ETag = the object hash when the file is a hardlinked artifact
        // (cheap to recover from the inode? no — from the catalog it would
        // need a lock). Keep the mtime+len tag: correct, and artifacts are
        // immutable anyway.
        serve_file(&fs_path, req, None)
    }

    fn api(&self, method: &str, rest: &str, req: &mut Request<'_>) -> Response {
        let segs: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        match (method, segs.as_slice()) {
            ("GET", ["tutorials"]) => {
                // Steps carry their live check outcome, evaluated now against
                // this depot (cheap: catalog/config already in memory).
                let d = self.depot.lock().unwrap();
                let list: Vec<serde_json::Value> = tutorial::all()
                    .into_iter()
                    .map(|t| {
                        let steps: Vec<serde_json::Value> = t
                            .steps
                            .iter()
                            .map(|st| {
                                let outcome = st.check.as_deref().and_then(|k| checks::run(k, &d));
                                json!({"title": st.title, "body": st.body, "say": st.say, "check": outcome})
                            })
                            .collect();
                        json!({"slug": t.slug, "title": t.title, "intro": t.intro, "steps": steps})
                    })
                    .collect();
                ok_json(&json!({"tutorials": list}))
            }
            ("POST", ["niftyplot"]) => {
                // Diagram source in, SVG out. Rendering is pure; the only
                // guard is the body size.
                let src = match req.read_body(256 * 1024) {
                    Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                    Err(e) => return Response::text(413, format!("niftyplot: {e}\n")),
                };
                Response::bytes(200, "image/svg+xml; charset=utf-8", nifty_core::svg::render_source(&src).into_bytes()).with_header("Cache-Control", "no-cache")
            }
            ("GET", ["tutorials", name]) => match name.strip_suffix(".md").and_then(tutorial::find) {
                Some(src) => Response::bytes(200, "text/markdown; charset=utf-8", src.text.as_bytes().to_vec()),
                None => Response::not_found(),
            },
            ("GET", ["status"]) => {
                let d = self.depot.lock().unwrap();
                match d.status() {
                    Ok(s) => ok_json(&s),
                    Err(e) => err(500, e),
                }
            }
            ("GET", ["crates"]) => {
                let q = req.query_param("q").unwrap_or_default().to_ascii_lowercase();
                let d = self.depot.lock().unwrap();
                let items: Vec<_> = d
                    .catalog
                    .crates
                    .iter()
                    .filter(|(n, _)| q.is_empty() || n.to_ascii_lowercase().contains(&q))
                    .take(500)
                    .map(|(n, vs)| {
                        let mut versions: Vec<&str> = vs.iter().map(|v| v.vers.as_str()).collect();
                        versions.sort_by(|a, b| depot_core::crates::semver_key(a).cmp(&depot_core::crates::semver_key(b)));
                        json!({"name": n, "versions": versions.len(), "latest": versions.last(), "yanked": vs.iter().filter(|v| v.yanked).count()})
                    })
                    .collect();
                ok_json(&json!({"crates": items, "total": d.catalog.crates.len()}))
            }
            ("GET", ["crates", name]) => {
                let d = self.depot.lock().unwrap();
                match d.catalog.crates.get(*name) {
                    Some(vs) => {
                        let mut vs: Vec<_> = vs.iter().map(|v| json!({"vers": v.vers, "hash": v.hash, "size": v.size, "yanked": v.yanked, "deps": v.index_line["deps"].as_array().map(|a| a.len()).unwrap_or(0), "path": format!("/crates/{}", depot_core::crates::dl_path(name, &v.vers))})).collect();
                        vs.sort_by(|a, b| depot_core::crates::semver_key(a["vers"].as_str().unwrap()).cmp(&depot_core::crates::semver_key(b["vers"].as_str().unwrap())));
                        ok_json(&json!({"name": name, "index": format!("/crates/index/{}", depot_core::crates::index_path(name)), "versions": vs}))
                    }
                    None => Response::not_found(),
                }
            }
            ("PUT", ["crates"]) => {
                if let Err(r) = self.authorize(req) {
                    return r;
                }
                let stored = match self.receive(req) {
                    Ok(s) => s,
                    Err(r) => return r,
                };
                let res = self.depot.lock().unwrap().ingest_crate_stored(&stored.hash);
                match res {
                    Ok(mut i) => {
                        i.fresh = stored.fresh;
                        ok_json(&i)
                    }
                    Err(e) => {
                        self.discard_if_fresh(&stored);
                        err(422, e)
                    }
                }
            }
            ("POST", ["crates", name, vers, action @ ("yank" | "unyank")]) => {
                if let Err(r) = self.authorize(req) {
                    return r;
                }
                match self.depot.lock().unwrap().yank_crate(name, vers, *action == "yank") {
                    Ok(()) => ok_json(&json!({"ok": true})),
                    Err(e) => err(404, e),
                }
            }
            ("GET", ["rpm"]) => {
                let d = self.depot.lock().unwrap();
                let repos: Vec<_> = d.catalog.rpm.iter().map(|(id, r)| json!({"id": id, "packages": r.packages.len(), "gpg_key": r.gpg_key.is_some(), "description": r.description, "bytes": r.packages.iter().map(|p| p.size).sum::<u64>()})).collect();
                ok_json(&json!({"repos": repos}))
            }
            ("GET", ["rpm", repo]) => {
                let q = req.query_param("q").unwrap_or_default().to_ascii_lowercase();
                let d = self.depot.lock().unwrap();
                match d.catalog.rpm.get(*repo) {
                    Some(r) => {
                        let mut pkgs: Vec<_> = r.packages.iter().filter(|p| q.is_empty() || p.name.to_ascii_lowercase().contains(&q)).map(|p| json!({"nevra": p.nevra(), "name": p.name, "arch": p.arch, "size": p.size, "hash": p.hash, "path": format!("/rpm/{repo}/{}", depot_core::views::rpm_href(&p.name, &p.filename))})).collect();
                        pkgs.sort_by(|a, b| a["nevra"].as_str().cmp(&b["nevra"].as_str()));
                        pkgs.truncate(1000);
                        ok_json(&json!({"id": repo, "description": r.description, "gpg_key": r.gpg_key.is_some(), "total": r.packages.len(), "packages": pkgs}))
                    }
                    None => Response::not_found(),
                }
            }
            ("PUT", ["rpm", repo]) => {
                if let Err(r) = self.authorize(req) {
                    return r;
                }
                let repo = repo.to_string();
                let stored = match self.receive(req) {
                    Ok(s) => s,
                    Err(r) => return r,
                };
                let res = self.depot.lock().unwrap().ingest_rpm_stored(&repo, &stored.hash, false);
                match res {
                    Ok(mut i) => {
                        i.fresh = stored.fresh;
                        ok_json(&i)
                    }
                    Err(e) => {
                        self.discard_if_fresh(&stored);
                        err(422, e)
                    }
                }
            }
            ("GET", ["nix"]) => {
                let d = self.depot.lock().unwrap();
                let items: Vec<_> = d.catalog.nix.iter().take(2000).map(|e| json!({"store_path": e.store_path, "narinfo": format!("/nix/{}.narinfo", e.hash_part), "nar": format!("/nix/{}", e.nar_url), "nar_size": e.nar_size})).collect();
                ok_json(&json!({"paths": items, "total": d.catalog.nix.len()}))
            }
            ("GET", ["toolchains"]) => {
                let d = self.depot.lock().unwrap();
                let items: Vec<_> = d.catalog.toolchains.iter().map(|t| json!({"version": t.version, "triple": t.triple, "component": t.component, "size": t.size, "path": format!("/toolchains/{}", depot_core::toolchain::served_path(t))})).collect();
                ok_json(&json!({"toolchains": items}))
            }
            ("PUT", ["toolchains"]) => {
                if let Err(r) = self.authorize(req) {
                    return r;
                }
                // The filename carries the metadata (component/version/triple),
                // so the client must tell us what it is uploading.
                let Some(fname) = req.header("x-filename").map(str::to_string) else {
                    return Response::text(400, "X-Filename: <component>-<version>-<triple>.tar.xz required\n");
                };
                if fname.contains('/') || fname.contains("..") {
                    return Response::text(400, "bad X-Filename\n");
                }
                let stored = match self.receive(req) {
                    Ok(s) => s,
                    Err(r) => return r,
                };
                let res = self.depot.lock().unwrap().ingest_toolchain_stored(&fname, &stored.hash);
                match res {
                    Ok(mut i) => {
                        i.fresh = stored.fresh;
                        ok_json(&i)
                    }
                    Err(e) => {
                        self.discard_if_fresh(&stored);
                        err(422, e)
                    }
                }
            }
            ("GET", ["have"]) => {
                let d = self.depot.lock().unwrap();
                match d.have() {
                    Ok(h) => Response::text(200, h.join("\n") + "\n"),
                    Err(e) => err(500, e),
                }
            }
            ("POST", ["pack"]) => {
                // Body = the receiver's have-list (may be empty). Query
                // `select=crates,nix,toolchains,rpm:<id>,rpm:*`. Response = the
                // pack, streamed from a temp file that is unlinked as soon as
                // it is open: Linux keeps an unlinked file readable through an
                // open descriptor, so nothing is left in tmp/ if the client
                // disconnects mid-download.
                let have: HashSet<String> = match req.read_body(64 * 1024 * 1024) {
                    Ok(b) => String::from_utf8_lossy(&b).lines().map(|l| l.trim().to_string()).filter(|l| depot_core::is_sha256_hex(l)).collect(),
                    Err(e) => return Response::text(413, format!("have-list: {e}\n")),
                };
                let sel = select_from_query(req.query_param("select").as_deref().unwrap_or("all"));
                let unsigned = req.query_param("unsigned").as_deref() == Some("1");
                let out = self.root.join("tmp").join(format!("pack-{}-{}.depot", std::process::id(), depot_core::now()));
                let res = self.depot.lock().unwrap().pack(&sel, &have, &out, unsigned);
                match res {
                    Ok(rep) => {
                        let f = match std::fs::File::open(&out) {
                            Ok(f) => f,
                            Err(e) => return Response::text(500, format!("open pack: {e}\n")),
                        };
                        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                        let _ = std::fs::remove_file(&out);
                        Response::new(200)
                            .with_header("Content-Type", "application/x-depot-pack")
                            .with_header("Content-Disposition", "attachment; filename=\"sync.depot\"")
                            .with_header("X-Depot-Objects", rep.objects.to_string())
                            .with_header("X-Depot-Skipped", rep.skipped_have.to_string())
                            .with_body(Body::File { file: f, len })
                    }
                    Err(e) => err(500, e),
                }
            }
            ("PUT", ["packs"]) => {
                if let Err(r) = self.authorize(req) {
                    return r;
                }
                let allow_unsigned = req.query_param("allow_unsigned").as_deref() == Some("1");
                let tmp = match self.spool(req, ".depot") {
                    Ok(p) => p,
                    Err(r) => return r,
                };
                let res = self.depot.lock().unwrap().unpack(&tmp, allow_unsigned);
                let _ = std::fs::remove_file(&tmp);
                match res {
                    Ok(rep) => ok_json(&json!({"signer": rep.signer, "objects_new": rep.objects_new, "objects_present": rep.objects_present, "bytes": rep.bytes, "crates_new": rep.crates_new, "rpm_new": rep.rpm_new, "nix_new": rep.nix_new, "toolchain_new": rep.toolchain_new})),
                    Err(e) => {
                        let code = if matches!(e, depot_core::Error::Untrusted(_)) { 403 } else { 422 };
                        err(code, e)
                    }
                }
            }
            ("GET", ["client-config"]) => {
                let want = want_from_query(req);
                let d = self.depot.lock().unwrap();
                let base = self.base_url(req);
                let snippets = clientcfg::snippets(&d, &base, &want);
                ok_json(&json!({"server": base, "snippets": snippets}))
            }
            ("GET", ["verify"]) => {
                let d = self.depot.lock().unwrap();
                match d.store.verify() {
                    Ok(bad) => ok_json(&json!({"corrupt": bad})),
                    Err(e) => err(500, e),
                }
            }
            _ => Response::not_found(),
        }
    }
}

impl depot_httpd::Handler for App {
    fn handle(&self, req: &mut Request<'_>) -> Response {
        self.handle_inner(req)
    }
}

fn ok_json<T: serde::Serialize>(v: &T) -> Response {
    match serde_json::to_string(v) {
        Ok(s) => Response::json(200, s).with_header("Cache-Control", "no-cache"),
        Err(e) => Response::text(500, format!("json: {e}\n")),
    }
}

fn err(status: u16, e: impl std::fmt::Display) -> Response {
    Response::json(status, json!({"error": e.to_string()}).to_string())
}

/// `?crates=1&rpm=fedora-42&rpm=updates&nix=1&toolchain=1`, or `?all=1`.
fn want_from_query(req: &Request<'_>) -> Want {
    let all = req.query_param("all").as_deref() == Some("1");
    let rpm: Vec<String> = req.query.split('&').filter_map(|kv| kv.split_once('=')).filter(|(k, _)| *k == "rpm").map(|(_, v)| depot_httpd::percent_decode(v)).filter(|v| !v.is_empty()).collect();
    Want { crates: all || req.query_param("crates").as_deref() == Some("1"), rpm, nix: all || req.query_param("nix").as_deref() == Some("1"), toolchain: all || req.query_param("toolchain").as_deref() == Some("1") }
}

/// `crates,nix,toolchains,rpm:fedora-42,rpm:*` or `all`.
pub fn select_from_query(s: &str) -> Select {
    if s == "all" || s.is_empty() {
        return Select::all();
    }
    let mut sel = Select::default();
    let mut repos = Vec::new();
    let mut any_rpm = false;
    for part in s.split(',') {
        match part.trim() {
            "crates" => sel.crates = true,
            "nix" => sel.nix = true,
            "toolchains" | "toolchain" => sel.toolchains = true,
            "rpm:*" | "rpm" => any_rpm = true,
            p if p.starts_with("rpm:") => repos.push(p[4..].to_string()),
            _ => {}
        }
    }
    if any_rpm {
        sel.rpm = Some(Vec::new());
    } else if !repos.is_empty() {
        sel.rpm = Some(repos);
    }
    sel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_query_forms() {
        assert!(select_from_query("all").crates);
        let s = select_from_query("crates,rpm:f42,rpm:f41");
        assert!(s.crates && !s.nix && s.rpm == Some(vec!["f42".into(), "f41".into()]));
        let s = select_from_query("rpm:*");
        assert_eq!(s.rpm, Some(vec![]));
        let s = select_from_query("nix,toolchains");
        assert!(s.nix && s.toolchains && s.rpm.is_none() && !s.crates);
    }
}
