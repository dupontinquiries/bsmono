//! chat_spatial — `Chat::spatial_ui`, the biggest method in `chat.rs`.
//!
//! Seventh cut. 2,707 lines: the scene, its drops, its panels and the transcript that sits in
//! front of them, in one function.
//!
//! ## Moved whole, not decomposed
//!
//! This is the method in its own file, not the method broken up. Those are different jobs and
//! only one is safe to do mechanically: the compiler proves a move, and it cannot prove that a
//! block lifted out of the middle did not depend on a local three hundred lines above it. The
//! decomposition is recorded in `TODO.md`.
//!
//! A CHILD module with `impl Chat`: the method needs `Chat`'s private fields and only a child
//! can see them. `use super::*` brings the types it borrows without re-importing each.

use super::*;

impl Chat {
    /// `pub(super)` rather than private: the method moved to a child module and its callers
    /// stayed in the parent, so "private to `chat`" now has to be spelled out. Not `pub` —
    /// this is still `chat`'s internal, living in another file.
    pub(super) fn spatial_ui(&mut self, ui: &mut egui::Ui, mut model: Option<&mut ModelState>) -> bool {
        let mut send_now = false;
        // DRAG-AND-DROP INGEST: files dropped onto the scene land in the active workspace session. This is
        // what makes the tmp workspace a real drop target — you can weave external files into the agent's
        // codebase by dropping them. Desktop drops carry a path; some stacks carry bytes; we handle both.
        if self.gitmind_session.is_some() {
            let drops = ui.ctx().input(|i| i.raw.dropped_files.clone());
            if !drops.is_empty() {
                let work_dir = self.gitmind_session.as_ref().unwrap().work_dir().to_path_buf();
                let mut ws = crate::workspace_exec::Workspace::new(work_dir);
                let mut ok = 0usize;
                for f in &drops {
                    let name = f.name.rsplit(['/', '\\']).next().filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .or_else(|| f.path.as_ref().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())))
                        .unwrap_or_else(|| "dropped.bin".into());
                    let dest = format!("dropped/{name}");
                    let res = if let Some(bytes) = &f.bytes {
                        ws.ingest(&dest, bytes)
                    } else if let Some(path) = &f.path {
                        match std::fs::read(path) { Ok(b) => ws.ingest(&dest, &b), Err(e) => crate::workspace_exec::ToolResult { ok: false, output: e.to_string() } }
                    } else {
                        crate::workspace_exec::ToolResult { ok: false, output: "empty drop".into() }
                    };
                    if res.ok { ok += 1; }
                }
                for ev in ws.take_events() { self.agent_files.push(ev); }
                self.note = format!("dropped {ok}/{} file(s) into the workspace → dropped/", drops.len());
                ui.ctx().request_repaint();
            }
        }

        let g = self.convo_graph(self.msgs.len());
        if g.nodes.is_empty() {
            if !self.backdrop {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.label(RichText::new("Spatial view").color(theme::DIM()).strong());
                ui.label(RichText::new("Nodes appear here as you converse — drag to orbit, scroll to zoom.").color(theme::FAINT()).small());
            });
            }
            return false;
        }
        let parents: Vec<Option<usize>> = g.nodes.iter().map(|n| n.parent).collect();
        let n = g.nodes.len();
        // River layout: messages flow across the scene (advance by log-time, branches take their own lanes),
        // spread out and meandering rather than coiled into a radial helix. Cached — recomputed on tree change.
        if self.fold_pos.len() != n {
            let t0 = self.msgs.first().map(|m| m.born);
            let times: Vec<f64> = (0..n).map(|i| self.msgs.get(i)
                .and_then(|m| t0.map(|t0| m.born.saturating_duration_since(t0).as_secs_f64()))
                .unwrap_or(i as f64)).collect();
            self.fold_pos = crate::spatial::flow(&parents, &times);
            // Per-node TOUR DWELL weight from message length: the camera lingers on longer messages (time to
            // read) and moves quicker past short ones. BOUNDED by a z-score clamp to [0.6, 2.4] so the pacing
            // stays even (the "bounded mean/stddev-wise to keep the flow" request) rather than crawling on one
            // huge turn or racing past a cluster of short ones. Indexed to match `pos` (g.nodes order).
            let lens: Vec<f32> = g.nodes.iter().map(|nd| self.msgs.get(nd.id).map(|m| m.text().chars().count() as f32).unwrap_or(0.0)).collect();
            let mean = if n > 0 { lens.iter().sum::<f32>() / n as f32 } else { 0.0 };
            let sd = if n > 0 { (lens.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / n as f32).sqrt().max(1.0) } else { 1.0 };
            self.tour_dwell = lens.iter().map(|&l| (1.0 + 0.6 * ((l - mean) / sd)).clamp(0.6, 2.4)).collect();
        }
        let pos = {
            let mut p = self.fold_pos.clone();
            // SEAT THE CONVERSATION ON THE FLOOR: the flow meander + sibling fanning can dip a node below a scene's
            // floor (≈ −3.2), so the strand pokes "into the ground". Lift the whole graph so its LOWEST node sits a
            // little above the floor — and only ever lift UP (never push a high graph down), so open scenes are
            // unaffected. This raises the walk to about eye level for the fly camera that rides the strand.
            let min_y = p.iter().map(|q| q.y).fold(f32::INFINITY, f32::min);
            if min_y.is_finite() {
                // Mistfall floats the conversation HIGH over its deep lake (the void between the low water and the
                // strand is what gives the cave its depth); other scenes seat just above their floor (~−3.2).
                let target_bottom = if theme::active().name == "Mistfall" { 1.6 } else { -1.6 };
                let lift = (target_bottom - min_y).max(0.0);
                for q in &mut p { q.y += lift; }
            }
            p
        };

        // HALL ROUTING: in hall-based scenes (Atlantis) the strand should run DOWN the open hallway lane along x,
        // not balloon into the furnished rotunda. Scale the graph laterally so its WIDEST node fits inside the clear
        // central corridor (between the colonnade rows + book-stacks), so it threads the open lane instead of the
        // columns. Scene-aware: round/open scenes keep their full spread. The geom harness then measures the drop.
        // Atlantis no longer squishes the strand laterally: its rotunda is now an OVAL that EXPANDS to contain the
        // graph's full branch span (see scene3d::atlantis + hall_dims), so the conversation spreads naturally rather
        // than being crammed onto the central axis. Other scenes already keep their full spread.

        // OBSTACLE FLEX: weave the strand AROUND solid scenery instead of clipping through it. The geom harness
        // showed tuning can't fix this (the strand balls up among thousands of obstacles), so push each node out
        // of any mid-size solid object it overlaps — along the smaller x/z penetration, biased to z so the time
        // axis (x) stays ordered, clamped near its spine spot. Cached per (node-count, env): the scene is
        // deterministic for a conversation, so this runs once, not per frame.
        let pos = {
            let key = (n, crate::scene3d::env_params());
            if self.flex_key.as_ref() != Some(&key) {
                let orig = pos.clone();
                let mut fp = pos.clone();
                let mut obs: Vec<([f32; 2], [f32; 2], [f32; 2])> = Vec::new(); // (x[lo,hi], z[lo,hi], y[lo,hi])
                crate::scene3d::with_env(theme::active().name, 0.0, |meshes| {
                    for m in meshes {
                        if m.verts.is_empty() { continue; }
                        let (mut blo, mut bhi) = (m.verts[0], m.verts[0]);
                        for v in &m.verts { blo.x = blo.x.min(v.x); blo.y = blo.y.min(v.y); blo.z = blo.z.min(v.z); bhi.x = bhi.x.max(v.x); bhi.y = bhi.y.max(v.y); bhi.z = bhi.z.max(v.z); }
                        let hx = (bhi.x - blo.x).max(bhi.z - blo.z); let vy = bhi.y - blo.y;
                        let fp_min = (bhi.x - blo.x).min(bhi.z - blo.z);
                        let hard = matches!(m.mat as u32, 1 | 2); // wood/stone only — leaves/water/paper are passable
                        if hard && hx < 14.0 && vy > 1.5 && fp_min > 0.4 { obs.push(([blo.x, bhi.x], [blo.z, bhi.z], [blo.y, bhi.y])); } // local solids, not terrain slabs
                    }
                });
                let r = 0.5f32; let cap = 8.0f32;
                for _ in 0..12 {
                    for (qi, q) in fp.iter_mut().enumerate() {
                        for (ox, oz, oy) in &obs {
                            if q.x + r > ox[0] && q.x - r < ox[1] && q.z + r > oz[0] && q.z - r < oz[1] && q.y > oy[0] - r && q.y < oy[1] + r {
                                let (px_lo, px_hi) = ((q.x + r) - ox[0], ox[1] - (q.x - r));
                                let (pz_lo, pz_hi) = ((q.z + r) - oz[0], oz[1] - (q.z - r));
                                // smallest penetration wins, but penalise x-moves (×1.6) so the strand goes AROUND
                                // sideways and keeps its time order along x
                                let cands = [(px_lo * 1.6, 0u8), (px_hi * 1.6, 1), (pz_lo, 2), (pz_hi, 3)];
                                let mut best = (f32::INFINITY, 4u8);
                                for &(v, a) in &cands { if v < best.0 { best = (v, a); } }
                                match best.1 { 0 => q.x -= px_lo, 1 => q.x += px_hi, 2 => q.z -= pz_lo, _ => q.z += pz_hi }
                            }
                        }
                        // keep each node near its spine position so the strand doesn't scatter
                        let (dx, dz) = (q.x - orig[qi].x, q.z - orig[qi].z);
                        let d = (dx * dx + dz * dz).sqrt();
                        if d > cap { q.x = orig[qi].x + dx / d * cap; q.z = orig[qi].z + dz / d * cap; }
                    }
                }
                self.flex_pos = fp; self.flex_key = Some(key);
            }
            if self.flex_pos.len() == n { self.flex_pos.clone() } else { pos }
        };

        // CANOPY HANG: in the Canopy scene the conversation DANGLES from the trees rather than resting among them.
        // A cone-scan upward from every third node finds a branch overhead and "ties a rope" — that node is pinned
        // up at branch height; the nodes between anchors then sag under gravity (catenary relaxation), so the strand
        // hangs in draped swags from the canopy instead of being squished flat by the surroundings. Distinct shape.
        let pos = if theme::active().name == "Canopy" && n > 2 {
            use crate::spatial::Vec3 as V3;
            let mut p = pos.clone();
            let floor = -3.2f32;
            // per-anchor branch height from a cone-scan: sample the canopy solids above the node and tie on just
            // beneath the lowest branch found (fall back to a default canopy height when the cone finds nothing).
            let mut obs: Vec<(V3, V3)> = Vec::new(); // (lo, hi) AABBs of canopy solids
            crate::scene3d::with_env("Canopy", 0.0, |meshes| {
                for m in meshes {
                    if m.verts.is_empty() || !matches!(m.mat as u32, 1 | 2 | 4) { continue; } // wood/stone/moss = branches & platforms
                    let (mut lo, mut hi) = (m.verts[0], m.verts[0]);
                    for v in &m.verts { lo.x = lo.x.min(v.x); lo.y = lo.y.min(v.y); lo.z = lo.z.min(v.z); hi.x = hi.x.max(v.x); hi.y = hi.y.max(v.y); hi.z = hi.z.max(v.z); }
                    if hi.y > floor + 3.0 { obs.push((lo, hi)); } // only things up in the canopy
                }
            });
            let cone = |x: f32, z: f32| -> f32 {                       // lowest branch overhead within a widening cone
                let mut best = f32::INFINITY;
                for (lo, hi) in &obs {
                    let r = 1.5 + (hi.y - floor) * 0.25;               // cone widens with height
                    if x > lo.x - r && x < hi.x + r && z > lo.z - r && z < hi.z + r && lo.y > floor + 2.0 { best = best.min(lo.y); }
                }
                if best.is_finite() { (best - 0.8).min(floor + 11.0) } else { floor + 7.5 } // tie on just under the branch
            };
            for (i, q) in p.iter_mut().enumerate() {
                if i % 3 == 0 { q.y = cone(q.x, q.z) + (((i as f32 * 12.9898).sin() * 43758.5453).fract().abs() - 0.5) * 1.2; } // anchor: roped to the branch
            }
            for _ in 0..60 {                                          // GRAVITY relaxation: chains sag between anchors
                for i in 1..p.len() - 1 {
                    if i % 3 == 0 { continue; }
                    let chord = (p[i - 1].y + p[i + 1].y) * 0.5;
                    p[i].y += (chord - 0.7 - p[i].y) * 0.5;           // pull below the chord → droop
                }
                for q in p.iter_mut() { if q.y < floor + 0.6 { q.y = floor + 0.6; } } // never sag through the floor
            }
            p
        } else { pos };

        // GEOMETRY DUMP (LOGSCOPE_GEOM_DUMP=1) for the geometric self-check harness (tools/geom_check.py): writes
        // scene object AABBs + graph node positions + the scene AABB to /tmp/scene_geom_<theme>.json, so bbox-
        // overlap + range tests can measure graph↔scene fit (into-the-ground, object clipping, out-of-bounds,
        // spine/axis alignment) objectively instead of eyeballing renders.
        if std::env::var("LOGSCOPE_GEOM_DUMP").is_ok() {
            use std::fmt::Write as _;
            let mut objs = String::new();
            let (mut smin, mut smax) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            crate::scene3d::with_env(theme::active().name, 0.0, |meshes| {
                for (i, m) in meshes.iter().enumerate() {
                    if m.verts.is_empty() { continue; }
                    let (mut lo, mut hi) = (m.verts[0], m.verts[0]);
                    for vtx in &m.verts {
                        lo.x = lo.x.min(vtx.x); lo.y = lo.y.min(vtx.y); lo.z = lo.z.min(vtx.z);
                        hi.x = hi.x.max(vtx.x); hi.y = hi.y.max(vtx.y); hi.z = hi.z.max(vtx.z);
                    }
                    smin[0] = smin[0].min(lo.x); smin[1] = smin[1].min(lo.y); smin[2] = smin[2].min(lo.z);
                    smax[0] = smax[0].max(hi.x); smax[1] = smax[1].max(hi.y); smax[2] = smax[2].max(hi.z);
                    let _ = write!(objs, "{}{{\"i\":{},\"lo\":[{:.3},{:.3},{:.3}],\"hi\":[{:.3},{:.3},{:.3}],\"mat\":{},\"emis\":{:.2}}}",
                        if objs.is_empty() { "" } else { "," }, i, lo.x, lo.y, lo.z, hi.x, hi.y, hi.z, m.mat as u32, m.emissive);
                }
            });
            let mut nodes = String::new();
            for (i, q) in pos.iter().enumerate() {
                let _ = write!(nodes, "{}{{\"i\":{},\"p\":[{:.3},{:.3},{:.3}]}}", if nodes.is_empty() { "" } else { "," }, i, q.x, q.y, q.z);
            }
            let json = format!(
                "{{\"theme\":\"{}\",\"nodes\":{},\"bounds\":{{\"lo\":[{:.2},{:.2},{:.2}],\"hi\":[{:.2},{:.2},{:.2}]}},\"objects\":[{}],\"graph\":[{}]}}",
                theme::active().name, pos.len(), smin[0], smin[1], smin[2], smax[0], smax[1], smax[2], objs, nodes);
            let path = format!("/tmp/scene_geom_{}.json", theme::active().name.replace(' ', "_"));
            let _ = std::fs::write(path, json);
        }

        let segs = g.segments();
        // PROCEDURAL THEMING: feed the conversation's shape to the environment generator so the hall is built
        // FROM the conversation — size from node count, colonnade from topic-segments, lanterns from branches,
        // and a content seed biases the tint. (Re-baked only when these change.)
        {
            let mut child_count: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
            for nd in &g.nodes { if let Some(pid) = nd.parent { *child_count.entry(pid).or_insert(0) += 1; } }
            let branches = child_count.values().filter(|&&v| v > 1).count() as u32;
            let seed = (n as u32).wrapping_mul(2654435761)
                ^ (segs.len() as u32).wrapping_mul(40503)
                ^ branches.wrapping_mul(97)
                ^ g.nodes.first().map(|nd| nd.preview.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32))).unwrap_or(0);
            // graph half-extents per axis (x runs along the spine, z across it) so the hall is sized AND
            // shaped to CONTAIN the flow — a long spine makes an oval rotunda elongated along x.
            let (ext_x, ext_z) = self.fold_pos.iter().fold((0.0_f32, 0.0_f32), |(mx, mz), p| (mx.max(p.x.abs()), mz.max(p.z.abs())));
            // PER-BRANCH BEARINGS: for each fork (a node with >1 child) take the direction from the fork to the
            // centroid of its NON-main-line descendants — i.e. which way that branch actually peels off. These
            // route the wings (region_plan) to the branch's real direction instead of an even spread.
            let bear = {
                let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
                for nd in &g.nodes { if let Some(p) = nd.parent { if p < n && nd.id < n { children[p].push(nd.id); } } }
                let p_at = |i: usize| self.fold_pos.get(i).copied().unwrap_or(crate::spatial::Vec3::new(0.0, 0.0, 0.0));
                let (mut size, mut sx, mut sz, mut cnt) = (vec![1usize; n], vec![0f32; n], vec![0f32; n], vec![0usize; n]);
                for i in 0..n { let p = p_at(i); sx[i] = p.x; sz[i] = p.z; cnt[i] = 1; }
                for i in (0..n).rev() { let ch = children[i].clone(); for c in ch { size[i] += size[c]; sx[i] += sx[c]; sz[i] += sz[c]; cnt[i] += cnt[c]; } }
                let mut mainline = vec![false; n];
                if n > 0 { let mut cur = 0usize; loop { mainline[cur] = true; if children[cur].is_empty() { break; } let nx = *children[cur].iter().max_by_key(|&&c| size[c]).unwrap(); if nx == cur { break; } cur = nx; } }
                let mut bears: Vec<f32> = Vec::new();
                for f in 0..n {
                    if children[f].len() > 1 {
                        let (mut ax, mut az, mut ac) = (0f32, 0f32, 0usize);
                        for &c in &children[f] { if !mainline[c] { ax += sx[c]; az += sz[c]; ac += cnt[c]; } }
                        if ac > 0 { let fp = p_at(f); let (bx, bz) = (ax / ac as f32 - fp.x, az / ac as f32 - fp.z); if bx * bx + bz * bz > 1e-4 { bears.push(bz.atan2(bx)); } }
                    }
                }
                let mut bear = [0u16; 6];
                for (i, &a) in bears.iter().take(6).enumerate() {
                    let frac = a.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
                    bear[i] = 1 + (frac * 65534.0) as u16;
                }
                bear
            };
            // PER-BRANCH SHAPE, in the same fork order as `bear` above — wing i is placed by
            // bear[i] and typed by shape[i], so the two must be taken in one order. Both are
            // ascending fork id: `bear` walks `for f in 0..n` and so does `branchshape::analyze`.
            let shape = {
                let parents: Vec<Option<usize>> = g.nodes.iter().map(|nd| nd.parent).collect();
                let toks: Vec<usize> = g.nodes.iter().map(|nd| nd.tokens).collect();
                let pruned: Vec<bool> = g.nodes.iter().map(|nd| nd.pruned).collect();
                let backs: Vec<bool> = g.nodes.iter().map(|nd| nd.looks_back).collect();
                let v = crate::branchshape::analyze(&parents, &toks, &pruned, &backs);
                let mut arr = [crate::branchshape::BranchShape::EMPTY; 6];
                for (i, s) in v.into_iter().take(6).enumerate() { arr[i] = s; }
                arr
            };
            let color_salience = chat_color_salience(self.msgs.iter().map(|m| m.text()));
            // procedural TREE character (Forest), seed-derived so each conversation grows a distinct wood, with a
            // light branchiness influence on the fork factor
            let tfrac = |salt: u32| -> f32 { let mut h = seed ^ salt.wrapping_mul(0x9E37_79B1); h ^= h >> 13; h = h.wrapping_mul(0xC2B2_AE35); h ^= h >> 16; (h & 0xFFFF) as f32 / 65535.0 };
            let tree_size = 0.93 + tfrac(0x71) * 0.14;        // 0.93..1.07 mild overall scale (compounds with h/g)
            let tree_height = 0.92 + tfrac(0x72) * 0.18;      // 0.92..1.10 trunk length factor (tall trees reach the upper strand → clip)
            let tree_girth = 0.85 + tfrac(0x73) * 0.40;       // 0.85..1.25 trunk radius factor (girth varies more freely)
            let tree_branch = (3 + branches / 8).clamp(3, 5); // only genuinely forky chats bushier (branches are obstacles)
            let branch_spread = 0.5 + tfrac(0x75) * 0.35;     // 0.50..0.85 second branching factor: how wide forks splay
            let prop_density = (0.75 + tfrac(0x76) * 0.5 + (n as f32 / 240.0).min(0.35)).clamp(0.7, 1.6); // street props scale with seed + size
            // THE BOARD SHOWS THE CONVERSATION. `scene3d::build` only ever had
            // the palette name, so the physical UI panel drew a stand-in in
            // every world. Segments are the real shape in scope here — a long
            // stretch of talk gets a wide card — and the scene cache keys on a
            // bucketed copy, so this churning every frame does not rebuild it.
            // AND WHICH TAB IS LIT. The plates along the board's top edge are the app's own
            // right-hand pages rendered as hardware, and the renderer lit the first one
            // unconditionally — so the board said Files while you were on Prompting. A
            // panel that reports the wrong state is worse than one with no tabs, because
            // the tabs are what say the board is live rather than scenery.
            let lit = match self.right_page {
                RightPage::Files => 0,
                RightPage::Graph => 1,
                RightPage::Context => 2,
                RightPage::Prompting => 3,
                // The skin draws four plates; Engine is a fifth page with no plate of its
                // own, so it lights the one it is nearest — Prompting — rather than an
                // index that does not exist.
                RightPage::Engine => 3,
            };
            crate::scene3d::set_panel_context(
                crate::panel_rows::PanelContext::from_rows(
                    // AND WHICH ROW IS LIVE. `active` was `None` at the only call site, so
                    // no row was ever accented and the renderer's emphasis path — brighter
                    // AND slightly taller, because at that distance brightness alone does
                    // not read — was dead code. The last segment is the one still being
                    // added to, which is the "you are here" a board in the room should show.
                    &crate::panel_rows::from_spans(&segs, segs.len().checked_sub(1)))
                    .with_active_tab(lit));
            crate::scene3d::set_env_params(crate::scene3d::EnvParams {
                nodes: n as u32, branches, segments: segs.len() as u32, seed,
                ext_x: ext_x.ceil() as u32, ext_z: ext_z.ceil() as u32, bear,
                color_salience, tree_size, tree_height, tree_girth, tree_branch, branch_spread, prop_density,
                shape,
            });
        }
        let mut seg_of = vec![0usize; n];
        for (si, &(a, b)) in segs.iter().enumerate() { for id in a..=b.min(n.saturating_sub(1)) { seg_of[id] = si; } }
        // SEGMENT COLOURS COME FROM THE ROOM. These were six fixed hues, identical in every space —
        // and the graph is the centre of the screen, so it was the most visible thing that did not
        // belong to the room it sat in. `nodetint` anchors them on the space's own accent and audits
        // every space for pair separation and contrast against its background.
        let palette = crate::nodetint::segments(&theme::active());
        let max_tok = g.nodes.iter().map(|nd| nd.tokens).max().unwrap_or(1).max(1) as f32;

        // (The 2D control bar was removed — Overview/Tour/Freecam/Prune/Unprune are physical dock buttons now.)

        let total = ui.available_size();
        // Full-bleed: the scene fills the whole window; the top control bar and the bottom read-out float over
        // it as translucent glass overlays (Areas), so nothing reserves vertical space any more.
        let canvas_h = total.y.max(180.0);
        let (resp, p) = ui.allocate_painter(egui::vec2(total.x, canvas_h), egui::Sense::click_and_drag());
        let rect = resp.rect;
        // Opaque base under the backdrop so empty/sky regions never reveal undefined framebuffer memory (garbage
        // on some drivers). The GPU composite also forces opacity; this also covers the CPU software path.
        if self.backdrop {
            let b = theme::active().bg;
            let bg = egui::Color32::from_rgb((b.r() as f32 * 0.7) as u8, (b.g() as f32 * 0.7) as u8, (b.b() as f32 * 0.7) as u8);
            p.rect_filled(rect, 0.0, bg);
        }
        let (cx, cy) = (rect.center().x, rect.center().y);
        let scale = rect.height().min(rect.width()) * 0.9;

        // auto-frame: target the graph's centroid and fit the distance, so it sits centered (the prior
        // bug was a fixed target leaving the strand off-frame). The user orbiting/zooming/clicking opts
        // out; "Overview" opts back in. With auto on, the strand stays centered as it grows (the protein).
        let c = crate::spatial::centroid(&pos);
        if self.spatial_auto {
            // EASE toward the centroid + fit distance rather than SNAPPING every frame. Snapping made the camera
            // twitch/ricochet as the strand breathes (centroid + bounding radius shift each frame); a gentle lerp
            // tracks it as a calm drift that combines the graph's motion with the framing of its bounds. Converges
            // in a few frames, so headless captures (which honour an explicit dist via spatial_zoomed) are unaffected.
            let k = 0.09;
            self.orbit.target.x += (c.x - self.orbit.target.x) * k;
            self.orbit.target.y += (c.y - self.orbit.target.y) * k;
            self.orbit.target.z += (c.z - self.orbit.target.z) * k;
            self.orbit.roll += (0.0 - self.orbit.roll) * k; // level the horizon when not in a cinematic shot
            if !self.spatial_zoomed {
                let fit = crate::spatial::fit_distance(crate::spatial::bounding_radius(&pos, c));
                self.orbit.dist += (fit - self.orbit.dist) * k;
            }
            ui.ctx().request_repaint(); // keep easing until settled
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            self.orbit.orbit(d.x * 0.01, -d.y * 0.01);
            ui.ctx().request_repaint();
        }
        if resp.hovered() {
            let sy = ui.input(|i| i.smooth_scroll_delta.y);
            if sy.abs() > 0.0 { self.orbit.zoom(1.0 - sy * 0.0015); self.spatial_zoomed = true; ui.ctx().request_repaint(); }
        }

        // PINCH-ZOOM — real multi-touch, which egui's ordinary Response drag/scroll path does not
        // model (that path only ever sees "the pointer", i.e. one contact). Gated the same as the
        // mouse controls above (hovered/dragged over the canvas) so a pinch starting on the composer
        // or a dock button doesn't also spin the camera. Only Pinch is applied here — a one-finger
        // Pan gesture is deliberately left alone since resp.dragged() already orbits from that same
        // touch, and running both would double the motion.
        if resp.hovered() || resp.dragged() {
            let now_ms = ui.input(|i| i.time) * 1000.0;
            ui.input(|i| {
                for ev in &i.events {
                    if let egui::Event::Touch { id, phase, pos, .. } = ev {
                        match phase {
                            egui::TouchPhase::Start | egui::TouchPhase::Move => { self.touch_active.insert(id.0, *pos); }
                            egui::TouchPhase::End | egui::TouchPhase::Cancel => { self.touch_active.remove(&id.0); }
                        }
                    }
                }
            });
            if self.touch_active.len() >= 2 {
                let touches: Vec<touchkit::Touch> = self.touch_active.iter()
                    .map(|(&id, p)| touchkit::Touch::new(id, touchkit::Vec2::new(p.x, p.y)))
                    .collect();
                for g in self.touch_rec.update(&touches, now_ms) {
                    if matches!(g, touchkit::Gesture::Pinch { .. }) {
                        crate::touch_nav::apply_gesture(&mut self.orbit, &g);
                        self.spatial_zoomed = true;
                        ui.ctx().request_repaint();
                    }
                }
            } else if !self.touch_active.is_empty() {
                // fewer than 2 contacts: nothing to pinch, but still feed the recognizer so it doesn't
                // see a phantom multi-touch gesture spanning a lift-off it never observed.
                let touches: Vec<touchkit::Touch> = self.touch_active.iter()
                    .map(|(&id, p)| touchkit::Touch::new(id, touchkit::Vec2::new(p.x, p.y)))
                    .collect();
                self.touch_rec.update(&touches, now_ms);
            }
        }

        // FREECAM: free-flight through the scene. The movement cluster comes from `navkeys` and
        // follows the selected keyboard layout — WASD + Q/E on QWERTY, `.oeu` + `,`/`p` on Dvorak —
        // with arrow keys always live in both. egui reports LOGICAL keys, so a Dvorak typist
        // pressing the physical W position sends `Comma` and, before this, simply did not move.
        // Drag looks (above), Shift boosts. Moving the target translates the eye with it (dist
        // held), so you fly wherever you look. Gated on hovering the canvas so the keys never leak
        // into the composer; repaints continuously to poll them.
        if self.freecam {
            use crate::spatial::Vec3 as V3;
            if resp.hovered() || resp.dragged() {
                let (mv, dt, boost) = ui.input(|i| {
                    let (cy, sy) = (self.orbit.yaw.cos(), self.orbit.yaw.sin());
                    let (cp, sp) = (self.orbit.pitch.cos(), self.orbit.pitch.sin());
                    let fwd = V3::new(cp * sy, -sp, -cp * cy);   // eye→target look direction (fly where you look)
                    let right = V3::new(cy, 0.0, sy);            // horizontal strafe
                    let (f, s, lift) = crate::navkeys::axes(self.nav_layout, |k| i.key_down(k));
                    let mut mv = V3::new(0.0, 0.0, 0.0);
                    mv.x += fwd.x * f + right.x * s;
                    mv.y += fwd.y * f + lift;
                    mv.z += fwd.z * f + right.z * s;
                    (mv, i.stable_dt.min(0.05), i.modifiers.shift)
                });
                if mv.x != 0.0 || mv.y != 0.0 || mv.z != 0.0 {
                    let speed = (if boost { 42.0 } else { 16.0 }) * dt;
                    self.orbit.target.x += mv.x * speed;
                    self.orbit.target.y += mv.y * speed;
                    self.orbit.target.z += mv.z * speed;
                    self.spatial_auto = false; self.spatial_zoomed = true;
                }
            }
            ui.ctx().request_repaint();
        }

        // VOICE DIRECTIONAL TRAVEL: "move back"/"move forward"/… drift the camera continuously along the
        // look basis until "stop"/"hold here" clears it. Same basis as freecam, but hands-free and steady.
        if let Some(dir) = self.cam_drift {
            use crate::spatial::Vec3 as V3;
            use crate::voice_intent::MoveDir;
            let (cy, sy) = (self.orbit.yaw.cos(), self.orbit.yaw.sin());
            let (cp, sp) = (self.orbit.pitch.cos(), self.orbit.pitch.sin());
            let fwd = V3::new(cp * sy, -sp, -cp * cy);   // eye→target
            let right = V3::new(cy, 0.0, sy);
            let v = match dir {
                MoveDir::Forward => fwd,
                MoveDir::Back => V3::new(-fwd.x, -fwd.y, -fwd.z),
                MoveDir::Right => right,
                MoveDir::Left => V3::new(-right.x, 0.0, -right.z),
                MoveDir::Up => V3::new(0.0, 1.0, 0.0),
                MoveDir::Down => V3::new(0.0, -1.0, 0.0),
            };
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            self.walk_ramp += (1.0 - self.walk_ramp) * (dt * 3.5).min(1.0); // ease the drift up to full over ~0.3s
            let speed = 14.0 * dt * self.walk_ramp;
            self.orbit.target.x += v.x * speed;
            self.orbit.target.y += v.y * speed;
            self.orbit.target.z += v.z * speed;
            self.spatial_auto = false;
            ui.ctx().request_repaint();
        }

        // WALKING BOB — a gentle footstep cadence on the view while drifting HORIZONTALLY (not up/down), so a
        // voice "walk"/"start moving" reads as walking rather than a dead-steady glide. The bob is eased in/out
        // and applied as a per-frame DELTA, so it never accumulates into a permanent vertical drift.
        {
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            let walking = self.cam_drift.map_or(false, |d| !matches!(d, crate::voice_intent::MoveDir::Up | crate::voice_intent::MoveDir::Down));
            let want = if walking { self.walk_phase += dt * 7.2; self.walk_phase.sin() * 0.05 } else { 0.0 };
            let eased = self.walk_bob + (want - self.walk_bob) * (dt * 6.0).min(1.0);
            self.orbit.target.y += eased - self.walk_bob;
            self.walk_bob = eased;
            if walking { ui.ctx().request_repaint(); }
        }
        // AUTOSCROLL WHILE STREAMING. A reply being generated grows the newest node, and without this the
        // camera sits wherever it was left and the user watches an empty region while the interesting
        // thing happens off-screen. Only engages when the user has NOT taken manual control
        // (`spatial_auto`) — following the tip is helpful during generation and infuriating if it fights
        // someone who is deliberately looking somewhere else.
        if self.stream.is_some() && self.spatial_auto && self.cam_pan_t0.is_none() {
            let tip = self.active_leaf.or_else(|| self.msgs.len().checked_sub(1));
            if let Some(id) = tip {
                if self.walk_focus != Some(id) && id < pos.len() {
                    let from = self.walk_focus.unwrap_or(id);
                    let path = graph_path(&g, from, id);
                    if path.len() > 1 { self.cam_pan_path = path; self.cam_pan_t0 = Some(std::time::Instant::now()); }
                    else { self.orbit.target = pos[id]; }
                    self.walk_focus = Some(id);
                    ui.ctx().request_repaint();
                }
            }
        }
        // path (parent links via LCA) — Catmull-Rom through the node positions — instead of jumping straight there.
        if let Some(t0) = self.cam_pan_t0 {
            let np = self.cam_pan_path.len();
            if np >= 2 {
                use crate::spatial::Vec3 as V3;
                let pp: Vec<V3> = self.cam_pan_path.iter().map(|&id| pos[id.min(pos.len() - 1)]).collect();
                let dur = 0.55 + 0.16 * (np as f32 - 1.0);          // longer, smoother glide along the path
                let raw = (t0.elapsed().as_secs_f32() / dur).clamp(0.0, 1.0);
                let e = raw * raw * (3.0 - 2.0 * raw);              // smoothstep ease
                let seg = e * (np - 1) as f32;
                let i0 = (seg.floor() as usize).min(np - 1);
                let i1 = (i0 + 1).min(np - 1);
                let fr = seg - i0 as f32;
                let g4 = |k: usize| pp[k.min(np - 1)];
                let cr = |p0: V3, p1: V3, p2: V3, p3: V3, u: f32| {
                    let (u2, u3) = (u * u, u * u * u);
                    let f = |a: f32, b: f32, c: f32, d: f32|
                        0.5 * ((2.0 * b) + (-a + c) * u + (2.0 * a - 5.0 * b + 4.0 * c - d) * u2 + (-a + 3.0 * b - 3.0 * c + d) * u3);
                    V3::new(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y), f(p0.z, p1.z, p2.z, p3.z))
                };
                let here = cr(g4(i0.saturating_sub(1)), g4(i0), g4(i1), g4((i1 + 1).min(np - 1)), fr);
                self.orbit.target = here;
                self.spatial_auto = false;
                ui.ctx().request_repaint();
                if raw >= 1.0 { self.cam_pan_t0 = None; self.cam_pan_path.clear(); }
            } else { self.cam_pan_t0 = None; }
        }

        // The fly-through is `fly_through` below: 212 lines of camera path that sat
        // between the node layout and the mesh draw, three unrelated things in a row.
        self.fly_through(ui, n, &pos);

        // A gently drifting camera (slight parallax + rotation) for projection & rendering — additive on top
        // of the persistent orbit, so it breathes without ever running off into a full orbit. Used for the
        // dots, hit-testing, and the 3D meshes alike, so everything stays aligned.
        let view_cam = self.orbit.drifted(ui.input(|i| i.time) as f32);

        // project every node, then sort far→near (painter's algorithm)
        // LOGSCOPE_WORLD_ONLY=1 renders just the ENVIRONMENT (no graph overlay/bodies/caption) — for pixel-level
        // matching of the world against a reference, with the conversation data layer suppressed.
        let world_only = std::env::var("LOGSCOPE_WORLD_ONLY").is_ok();
        let mut proj: Vec<(usize, f32, f32, f32)> = Vec::with_capacity(n);
        for nd in &g.nodes {
            if let Some((sx, sy, depth)) = view_cam.project(pos[nd.id], cx, cy, scale) {
                // GUARD: a node landing on/behind the camera plane (e.g. the tour's close push-in) can project to
                // a non-finite depth; if it enters `proj` it poisons near/far/span via the fold, turning every
                // depth-normalized colour factor into NaN → Color32 multiply panics ("tour breaks the render").
                if sx.is_finite() && sy.is_finite() && depth.is_finite() {
                    proj.push((nd.id, sx, sy, depth));
                }
            }
        }
        proj.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
        if world_only { proj.clear(); }  // suppress the 2D dot/cord/rune/label overlay
        let screen = |id: usize| proj.iter().find(|q| q.0 == id).map(|q| (q.1, q.2));

        // 3D mesh layer — each node a lit body (stones for Mistfall/Slate, leaf-puffs for Canopy, orbs
        // otherwise), strung on rippling strands (vine / water / air perturbation), shaded by a clock-driven
        // sun, drawn with the SAME camera as the dots. The dots/labels below sit on top for hit-testing.
        {
            use crate::mesh3d as m3;
            m3::set_tess_mul(theme::detail(theme::variant()).3); // detail variant scales all object tessellation
            let t = ui.input(|i| i.time) as f32;
            let name = theme::active().name;
            // PER-SCENE LIGHTING: Light::sun() only hands out a generic fill (fixed sky/ground/ambient) — so
            // every scene read the same. Give each its own sky/ground bounce + ambient + an exposure trim so the
            // spaces have distinct moods (aqua Atlantis · dappled Canopy · misty Mistfall · warm Archive ·
            // bright-airy Reading Room/Cloudscape · clinical Folding).
            let (lsky, lground, lamb, lexp) = scene_light(name);
            let mut light = m3::Light::sun(m3::sun_dir_3d(), theme::active().light);
            light.sky = lsky; light.ground = lground; light.ambient = lamb;
            let style = match name {
                "Canopy" => m3::NodeStyle::Leaf,
                "Mistfall" | "Slate" => m3::NodeStyle::Stone,
                "Atlantis" => m3::NodeStyle::Pearl,   // glossy prismarine pearls
                _ => m3::NodeStyle::Orb,
            };
            let edge_col = theme::LINE();
            let (strand_mat, node_mat) = match name { // strands = vine/water/stone; bodies = foliage/stone
                "Canopy" => (crate::texgen::mat::PAPER, crate::texgen::mat::MOSS), // air-field cords + leaf bodies
                "Mistfall" => (crate::texgen::mat::WATER, crate::texgen::mat::STONE),
                "Folding@home" => (crate::texgen::mat::PAPER, crate::texgen::mat::PAPER),
                "Atlantis" => (crate::texgen::mat::WATER, crate::texgen::mat::WATER), // glassy cords + glassy pearls
                "Treehouse" => (crate::texgen::mat::WOOD, crate::texgen::mat::WOOD),  // rope cords + carved wood balls
                _ => (crate::texgen::mat::STONE, crate::texgen::mat::STONE),
            };
            // Atlantis renders the graph as a beam array: pearls glow softly and the strands between them are
            // bright, near-straight LIGHT BEAMS (additive → caught by bloom). Mistfall keeps soft WATER cords;
            // other themes use plain edges.
            let beam = name == "Atlantis";
            let (cord_r, cord_amp, cord_glow, cord_seg) = if beam { (0.024, 0.05, 1.1, 7usize) }
                else if name == "Mistfall" { (0.05, 0.34, 0.0, 13) }
                else if name == "Canopy" { (0.02, 0.5, 0.30, 14) } // a vibrating AIR filament that holds the leaves
                else { (0.045, 0.18, 0.0, 7) };
            let pearl_glow = if name == "Atlantis" { 0.14 } else { 0.0 };
            // Mistfall: the floating stones carry a CARVED BLUE RUNE glow — one colour unified across the whole
            // graph (so the conversation reads as a single spell), shifted slightly by the env variant.
            let rune = name == "Mistfall";
            // Treehouse: the graph reads as carved WOOD BALLS hung by rope from the canopy overhead — each node
            // is a wooden orb with a rope rising above it (the "inscription" stays as the node's label).
            let treehouse = name == "Treehouse";
            let rune_col = {
                let h = (theme::variant() as u32).wrapping_mul(0x9E3779B9) >> 25; // 0..127 from the variant
                egui::Color32::from_rgb(0x3c + (h % 24) as u8, 0x96, 0xd8 + (h % 20) as u8)
            };
            let cord_col = if beam { egui::Color32::from_rgb(0xb6, 0xee, 0xff) } else if name == "Canopy" { egui::Color32::from_rgb(0xd8, 0xe4, 0xe8) } else if name == "Frozen Bifrost" { egui::Color32::from_rgb(0x9a, 0xb0, 0xe0) } else { edge_col };
            let mut graph: Vec<m3::Mesh3> = Vec::with_capacity(n * 2);
            let tanim = text_anim_for(name); // per-scene text-animation idiom for all scene/UI labels
            let sheen = slab_sheen(tanim, t); // per-scene light-play multiplier for physical UI slabs
            let sun = m3::sun_dir_3d(); // world sun direction, for specular glints on physical slabs
            // PEARL PULL — ease focus_pull toward 1 while a turn is focused, toward 0 otherwise. An
            // exponential approach (not a linear lerp) so the pull decelerates into the reading pose
            // instead of arriving at a constant speed — same shape as the plaque's scroll glide.
            let focused_id = std::env::var("LOGSCOPE_FOCUS").ok().and_then(|s| s.trim().parse::<usize>().ok()).or(self.walk_focus).filter(|i| *i < n);
            // PLAQUE TOUCH OVERRIDE — physical-ui-findings.md's open item: a click ON the reading
            // plaque should light up the plaque (a real object at its own OVERLAY-biased world
            // position), not whatever the floor/graph raycast happens to hit at that screen pixel
            // behind it. Set below (inside the plaque's own hit-test, which knows `rc`) and consumed
            // by the generic scene-touch handler further down, which otherwise always wins on a raw
            // press. `None` means "no plaque claimed this press" → fall through to the raycast as before.
            let mut touch_override: Option<[f32; 3]> = None;
            // HUD PACKING — `hud_pack` (screen-space overlap resolution, its own 8 tests including the
            // exact "bottom trio" collision below) existed fully built and was never called anywhere in
            // the crate; the physical dock/composer/action-row conflict its header describes as already
            // reported was still live. One shared list, filled in the SAME order these panels already
            // render in (call order = priority, per hud_pack's own contract) — dock first, so a full
            // window keeps the dock where it's always been, and the composer/action row slide out of its
            // way on short windows instead of overlapping it.
            let mut hud_placed: Vec<crate::hud_pack::Placed> = Vec::new();
            {
                let dt = ui.input(|i| i.stable_dt).min(0.1);
                let target = if focused_id.is_some() { 1.0 } else { 0.0 };
                let rate = 8.0; // higher = snappier settle; matches the walk-camera's damping feel
                self.focus_pull += (target - self.focus_pull) * (1.0 - (-rate * dt).exp());
                if (self.focus_pull - target).abs() > 0.002 { ui.ctx().request_repaint(); }
            }
            let eye_now = view_cam.eye_world();
            // VOICE LIVE-INTENT HIGHLIGHT: while an utterance is being heard, parse the running PARTIAL — if it
            // already resolves to an ACTIONABLE command (not plain dictation) we light up the mic (below), and
            // for "go to N" we glow the target node, so the user sees what will happen before whisper finalizes.
            // A debug env var lets the headless harness inject a partial for render verification.
            let voice_partial = self.voice.as_ref().map(|v| v.partial().to_string())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| std::env::var("LOGSCOPE_VOICE_PARTIAL").ok().filter(|s| !s.trim().is_empty()));
            let (voice_actionable, voice_target): (bool, Option<usize>) = match voice_partial.as_deref().map(crate::voice_intent::parse) {
                None | Some(crate::voice_intent::VoiceIntent::Dictate(_)) => (false, None),
                Some(crate::voice_intent::VoiceIntent::Command(crate::vimmode::Command::Goto(n))) => (true, Some(n.saturating_sub(1))),
                Some(_) => (true, None),
            };
            for nd in &g.nodes {
                if let Some(pid) = nd.parent {
                    let mut st = m3::with_mat(m3::strand(pos[pid], pos[nd.id], cord_r, 6, cord_seg, t, cord_amp, cord_col), strand_mat);
                    st.emissive = cord_glow;
                    if name == "Canopy" { m3::set_alpha(&mut st, 85); } // faint, see-through air
                    if name == "Frozen Bifrost" { m3::set_alpha(&mut st, 74); st.emissive = 0.6; } // faint glowing constellation lines between stars
                    graph.push(st);
                }
            }
            for nd in &g.nodes {
                let active = self.active_leaf == Some(nd.id);
                let wood = [egui::Color32::from_rgb(0x8c, 0x66, 0x3c), egui::Color32::from_rgb(0x74, 0x52, 0x2e), egui::Color32::from_rgb(0x9c, 0x74, 0x46), egui::Color32::from_rgb(0x6a, 0x4a, 0x2a)];
                let col = if active { theme::accent() } else if rune { rune_col } else if treehouse { wood[nd.id % 4] } else { palette[seg_of[nd.id] % palette.len()] };
                let mut nm = if name == "Old Library" {
                    // a turn's FORM follows its content: short snippet → page, code/file/long → scroll (the
                    // sheet unrolls further the longer it is), otherwise a closed book. Code/file is sniffed
                    // from the preview (fences, braces, calls, a path with an extension).
                    let p = &nd.preview;
                    let codeish = p.contains('`') || p.contains('{') || p.contains(';') || p.contains("fn ")
                        || p.contains("def ") || p.contains("()") || p.contains("::") || (p.contains('/') && p.contains('.'));
                    let (kind, unroll) = if codeish || nd.tokens > 180 {
                        (2u32, (nd.tokens as f32 / 420.0).clamp(0.18, 1.0))
                    } else if nd.tokens < 42 { (0u32, 0.0) } else { (1u32, 0.0) };
                    m3::with_mat(m3::library_node(kind, pos[nd.id], 0.26, unroll, nd.id as u32, col), crate::texgen::mat::PAPER)
                } else if name == "Atlantis" {
                    // smooth nacreous PEARLS (the cracked-gem node read as a foreign asset / wrong texture);
                    // node_mat = WATER → the nacre/thin-film shading gives the pearly sheen.
                    m3::with_mat(m3::uv_sphere(pos[nd.id], 0.27, 22, 14, col), node_mat)
                } else if name == "Frozen Bifrost" {
                    // ASGARD: each turn is a STAR in the sky off the world — a small bright emissive point (the
                    // twinkle is applied to emissive below), rendered in the space void the scene sets up.
                    m3::with_mat(m3::uv_sphere(pos[nd.id], 0.15, 10, 8, col), node_mat)
                } else {
                    // Slate reads too uniform → vary the stone SIZE by the turn's weight (on top of the
                    // per-seed shape variation rock() already gives); other scenes keep a steady node size.
                    let nr = if name == "Slate" { 0.17 + 0.20 * (nd.tokens as f32 / 150.0).clamp(0.0, 1.4) } else { 0.26 };
                    m3::with_mat(m3::node_mesh(style, pos[nd.id], nr, nd.id as u32, col), node_mat)
                };
                if name == "Atlantis" {
                    // pearls drift SLOWLY on the currents instead of sitting rigid on the beams (the offset is
                    // applied to the packed verts each frame, so they bob gently as the UI repaints).
                    let ph = nd.id as f32 * 1.7;
                    nm.offset = crate::spatial::Vec3::new(0.04 * (t * 0.30 + ph).sin(), 0.09 * (t * 0.24 + ph * 1.3).sin(), 0.04 * (t * 0.28 + ph * 0.7).cos());
                }
                // PEARL PULL: the focused turn's pearl leaves its strand and drifts toward the camera,
                // eased in step with `focus_pull`. Reuses the same per-frame `offset` mechanism as the
                // Atlantis drift above (additive, so the two combine rather than fight), just along the
                // node→eye vector instead of a current. Distance is capped well short of the near plane
                // so the pearl never clips into the lens even at max pull.
                if focused_id == Some(nd.id) && self.focus_pull > 0.001 {
                    let p = pos[nd.id];
                    let to_eye = crate::spatial::Vec3::new(eye_now.x - p.x, eye_now.y - p.y, eye_now.z - p.z);
                    let len = (to_eye.x * to_eye.x + to_eye.y * to_eye.y + to_eye.z * to_eye.z).sqrt().max(1e-4);
                    let pull_dist = (view_cam.dist * 0.35).min(2.6) * self.focus_pull;
                    let (dx, dy, dz) = (to_eye.x / len * pull_dist, to_eye.y / len * pull_dist, to_eye.z / len * pull_dist);
                    nm.offset = crate::spatial::Vec3::new(nm.offset.x + dx, nm.offset.y + dy, nm.offset.z + dz);
                    // NOTE: the glow rise used to be set HERE via nm.emissive.max(...), but the very next
                    // statement below unconditionally overwrites nm.emissive (rune/star/base glow), so it
                    // was clobbered every frame and never actually rendered. Moved past that cascade —
                    // see the `.max()` call after the vhi pulse below.
                }
                // HOVER / PRESSED — a node the pointer is over lifts/brightens (Hover), and one actually
                // being held down sinks/dims instead (Pressed) — the two `scenepick::State`s this scene
                // renders, chosen so a press reads as "pushed in" rather than just "still hovered".
                // Skipped on the focused/pulled node so the effects don't fight over the same offset.
                if focused_id != Some(nd.id) {
                    if self.scene_pressed == Some(nd.id) {
                        let st = crate::scenepick::State::Pressed;
                        nm.offset = crate::spatial::Vec3::new(nm.offset.x, nm.offset.y + st.lift(), nm.offset.z);
                    } else if self.scene_hover == Some(nd.id) {
                        let st = crate::scenepick::State::Hover;
                        nm.offset = crate::spatial::Vec3::new(nm.offset.x, nm.offset.y + st.lift(), nm.offset.z);
                    }
                }
                nm.emissive = if rune { 0.40 + 0.10 * (t * 1.3 + nd.id as f32 * 0.5).sin() }
                    else if name == "Frozen Bifrost" { 2.2 + 0.9 * (t * 2.0 + nd.id as f32 * 1.3).sin() } // stars twinkle
                    else { pearl_glow };
                if active { nm.emissive = 0.9 + 0.35 * (t * 1.6).sin(); }
                let vhi = voice_target == Some(nd.id);
                if vhi { nm.emissive = 1.0 + 0.5 * (t * 6.0).sin(); } // the node the user is asking for — pulse hot
                // pull glow + hover brightening, applied LAST so they ride on top of whatever the base/
                // active/vhi cascade above landed on, instead of being computed then immediately lost.
                if focused_id == Some(nd.id) { nm.emissive = nm.emissive.max(0.35 * self.focus_pull); }
                if focused_id != Some(nd.id) {
                    if self.scene_pressed == Some(nd.id) {
                        let (bright, _sat) = crate::scenepick::State::Pressed.tint();
                        nm.emissive *= bright; // dims — "pushed in", not lit up
                    } else if self.scene_hover == Some(nd.id) {
                        let (bright, _sat) = crate::scenepick::State::Hover.tint();
                        nm.emissive = (nm.emissive + 0.12) * bright;
                    }
                }
                graph.push(nm);
                if vhi {
                    // a bright accent RING around the spoken-of node, so "go to twelve" visibly marks node 12 live
                    let a = theme::accent();
                    let mut ring = m3::with_mat(m3::torus(pos[nd.id], 0.44, 0.03, 20, 6, a), node_mat);
                    ring.emissive = 1.4 + 0.6 * (t * 6.0).sin(); graph.push(ring);
                }
                // ASGARD: cross-shaped GLINT rays off each star, so the nodes read as sparkling points of light.
                if name == "Frozen Bifrost" {
                    let p = pos[nd.id];
                    let gl = 0.34 + 0.12 * (t * 2.0 + nd.id as f32).sin();
                    for (dx, dz) in [(gl, 0.0), (0.0, gl)] {
                        let a = crate::spatial::Vec3::new(p.x - dx, p.y, p.z - dz);
                        let b = crate::spatial::Vec3::new(p.x + dx, p.y, p.z + dz);
                        let mut ray = m3::with_mat(m3::tube(a, b, 0.012, 3, col), node_mat);
                        ray.emissive = 1.6; graph.push(ray);
                    }
                    // a faint vertical glint too (a 4-point star)
                    let mut vray = m3::with_mat(m3::tube(crate::spatial::Vec3::new(p.x, p.y - gl, p.z), crate::spatial::Vec3::new(p.x, p.y + gl, p.z), 0.012, 3, col), node_mat);
                    vray.emissive = 1.6; graph.push(vray);
                }
                // Treehouse: each wood ball HANGS from the overhead canopy — a sagging rope rising to a canopy
                // anchor KNOT (varied height, so ropes reach real branch level), with a couple of splay web
                // strands hooking the anchor into the surrounding foliage (not a stub ending in mid-air).
                if treehouse {
                    let p = pos[nd.id];
                    let canopy_y = p.y + 3.4 + 0.6 * (nd.id as f32 * 1.7).sin();
                    let anchor = crate::spatial::Vec3::new(p.x + 0.3 * (nd.id as f32).sin(), canopy_y, p.z + 0.3 * (nd.id as f32).cos());
                    let rope_col = egui::Color32::from_rgb(0x46, 0x34, 0x20);
                    let mut rope = m3::with_mat(m3::strand(anchor, p, 0.022, 4, 6, t, 0.10, rope_col), crate::texgen::mat::WOOD);
                    if active { rope.emissive = 0.2; }
                    graph.push(rope);
                    let knot = m3::with_mat(m3::uv_sphere(anchor, 0.09, 6, 5, egui::Color32::from_rgb(0x3a, 0x2a, 0x18)), crate::texgen::mat::WOOD);
                    graph.push(knot);
                    for wi in 0..2 { // splay webs hooking the anchor into the canopy branches
                        let wa = wi as f32 * std::f32::consts::PI + nd.id as f32;
                        let out = crate::spatial::Vec3::new(anchor.x + wa.cos() * 0.9, anchor.y + 0.45, anchor.z + wa.sin() * 0.9);
                        graph.push(m3::with_mat(m3::tube(anchor, out, 0.012, 3, rope_col), crate::texgen::mat::WOOD));
                    }
                }
            }

            // pearl-reader: starry speckle cells inside NEAR pearls (LOD by projected depth), twinkling and
            // brightening as the camera approaches (resonance) — the living point-cloud pearl, in 3D.
            if name == "Atlantis" {
                let (mut mind, mut maxd) = (f32::MAX, f32::MIN);
                for q in &proj { mind = mind.min(q.3); maxd = maxd.max(q.3); }
                let span = (maxd - mind).max(1e-3);
                for q in &proj {
                    let near = ((maxd - q.3) / span).clamp(0.0, 1.0); // 1 = nearest the camera
                    if near < 0.4 { continue; }
                    let ncells = (near * near * 34.0) as usize;
                    if ncells < 5 { continue; }
                    let active = self.active_leaf == Some(q.0);
                    let ccol = if active { theme::accent() } else { theme::CYAN() };
                    for cell in m3::speckle_cells(pos[q.0], 0.26, q.0 as u32, t, ccol, ncells, near) {
                        graph.push(cell);
                    }
                }

                // text-on-approach: the NEAREST pearls resolve their turn label into glowing cells (resonance) —
                // the speckle is the body, the legible text condenses out of it as the camera draws close.
                let (vy, vp) = (view_cam.yaw, view_cam.pitch);
                let (cyaw, syaw) = (vy.cos(), vy.sin());
                let (cp, sp) = (vp.cos(), vp.sin());
                let right = crate::spatial::Vec3::new(cyaw, 0.0, syaw);
                let up = crate::spatial::Vec3::new(syaw * sp, cp, -cyaw * sp);
                let toward_cam = crate::spatial::Vec3::new(-(cp * syaw), sp, cp * cyaw); // -viewDir → toward eye
                let scroll = (t * 0.12).sin() * 0.06; // gentle teleprompter drift
                // WHICH LABELS GET DRAWN, decided on the label BOXES rather than the pearl bodies.
                // The dimming pass below is about occlusion by a nearer node; it cannot see two
                // labels of similar depth sitting side by side, which is how a cluster of turns
                // ends up as one smear of overlapping text. `nodelabel::visible` keeps the nearest
                // and drops what would collide with it.
                const LABEL_CHARS: usize = 22;
                // WHAT EACH NODE IS, above what it says — the information architecture of the
                // reference cards, whose every plaque carries a type over its content line. Derived
                // from the graph (pruned, root, fork, loopback, artifact) rather than from the
                // reference's own vocabulary, which belongs to a worked example rather than to this
                // data; see `noderole`. The common case is left untagged.
                let node_roles = crate::noderole::roles(&g);
                let label_of = |id: usize| -> String {
                    match (g.nodes.iter().position(|nd| nd.id == id), g.nodes.iter().find(|nd| nd.id == id)) {
                        (Some(i), Some(nd)) => crate::noderole::line(
                            node_roles.get(i).copied().unwrap_or(crate::noderole::Role::Reply),
                            &nd.preview, LABEL_CHARS),
                        _ => String::new(),
                    }
                };
                // LABELS RING THE CLUSTER rather than sitting on it — what the references actually
                // do, and the better answer to crowding than hiding the losers: a label you cannot
                // see is a turn you cannot read. Pushed radially from the graph's own centroid,
                // which is the direction with room in it. The declutter below then runs on these
                // moved positions and has far less to resolve.
                // IN WORLD SPACE, not screen space. Pushing the screen anchor would decide
                // visibility from boxes at one place while `label_cells` drew the text at another —
                // a declutter measuring positions the labels do not occupy, which is the same
                // one-step-removed mistake as auditing a palette that never renders. Offsetting the
                // world point and projecting THAT keeps the box and the glyphs on the same spot.
                const LABEL_PUSH_WORLD: f32 = 0.55;
                // ONE CELL SIZE. `label_cells` lays 5x7 cells of this size and `px_extents` derives
                // the collision box from it, so the number was written twice — once for the glyphs
                // and once for the box. Change either and the declutter measures a rectangle the
                // text does not occupy, silently. That is the two-copies-of-one-value defect this
                // tree has shipped four times; it does not get a fifth.
                const LABEL_CELL: f32 = 0.034;
                // `spatial::centroid` already means this. I hand-rolled the x/z mean here and
                // dupscan named it; that is the third copy of an existing helper this session.
                let world_mid = crate::spatial::centroid(&pos);
                // The rule lives in `nodelabel::outward` so the label text and its connector line
                // read the SAME function and cannot disagree about where the label is. Inlining it
                // here left the module's own copy with no callers — dead code written in the commit
                // that fixed the previous mistake.
                let label_pos = |id: usize| crate::nodelabel::outward(pos[id], world_mid, LABEL_PUSH_WORLD);
                let anchor = |q: &(usize, f32, f32, f32)| -> (f32, f32) {
                    view_cam.project(label_pos(q.0), cx, cy, scale)
                        .map(|(sx, sy, _)| (sx, sy)).unwrap_or((q.1, q.2))
                };
                let show: std::collections::HashSet<usize> = {
                    let placed: Vec<crate::nodelabel::LabelBox> = proj.iter().filter_map(|q| {
                        let _nd = g.nodes.iter().find(|nd| nd.id == q.0)?;
                        let chars = label_of(q.0).chars().count();
                        // EXTENTS FROM THIS LABEL'S OWN DEPTH, using the same `scale` the projection
                        // above used. The previous version passed a flat 7px advance, which asserts
                        // a label ten metres away is as wide on screen as one at arm's length —
                        // over-suppressing far labels and under-suppressing near ones, the two cases
                        // the declutter exists to get right. LABEL_CELL is `label_cells`' own cell
                        // size at this call site.
                        let (w, h) = crate::nodelabel::px_extents(chars, LABEL_CELL, scale, q.3);
                        let (ax, ay) = anchor(q);
                        Some(crate::nodelabel::LabelBox { id: q.0, x: ax, y: ay, depth: q.3, w, h })
                    }).collect();
                    crate::nodelabel::visible(placed).into_iter().collect()
                };
                for q in &proj {
                    if !show.contains(&q.0) { continue; }
                    let near = ((maxd - q.3) / span).clamp(0.0, 1.0);
                    let mut lit = ((near - 0.80) / 0.18).clamp(0.0, 1.0); // resolves only when very close
                    if lit < 0.05 { continue; }
                    // occlusion dimming: if a NEARER pearl overlaps this one on screen, fade this label back so
                    // the front one reads cleanly (declutters stacked labels along the strand).
                    let mut occ = 0.0f32;
                    for r2 in &proj {
                        if r2.3 < q.3 - 0.1 {
                            let (dx, dy) = (r2.1 - q.1, r2.2 - q.2);
                            if dx * dx + dy * dy < 78.0 * 78.0 { occ += 1.0; }
                        }
                    }
                    lit /= 1.0 + 1.5 * occ;
                    if lit < 0.05 { continue; }
                    if let Some(nd) = g.nodes.iter().find(|nd| nd.id == q.0) {
                        // Word-safe, and prefixed with what the node IS when that is worth saying.
                        let _ = nd;
                        let label = label_of(q.0);
                        if label.is_empty() { continue; }
                        // A CONNECTOR, because the label no longer sits on its node. Pushing them
                        // outward without a line leaves text floating with nothing tying it to the
                        // turn it describes — the references always draw the leader. `m3::tube`
                        // already exists for this; a second line primitive would have been the
                        // fourth duplication this session.
                        {
                            let (a, b) = (pos[q.0], label_pos(q.0));
                            let mut lead = m3::tube(a, b, 0.012, 4, theme::lerp_col(theme::CYAN(), theme::PANEL(), 0.45));
                            lead.overlay = 1.0;
                            graph.push(lead);
                        }
                        for cell in m3::label_cells(label_pos(q.0), right, up, toward_cam, &label,
                                                    LABEL_CELL, t, theme::CYAN(), lit, scroll, tanim, m3::SceneLight::Uniform, 0) {
                            graph.push(cell);
                        }
                    }
                }
            }

            // Reflection — the water (or polished floor) holds a faded image of the conversation. Each node
            // and strand is mirrored onto the surface plane, dimmer where the real node rides higher (a
            // glancing reflection) and dimmer with age, so the past softens into the surface as "memory".
            // (A blended ghost on the surface; true sub-surface refraction would want a translucent-water
            // pass — a follow-up.)
            let (refl_amt, plane_y) = match name {
                "Mistfall" => (0.62_f32, -3.55_f32), // water — strong
                "Reading Room" => (0.30, -2.95),     // polished limestone — subtle
                "Canopy" | "Archive" => (0.20, -3.15),
                "Folding@home" => (0.0, 0.0),        // no reflective plane near the helix
                _ => (0.26, -2.95),
            };
            if refl_amt > 0.0 {
                let bg = theme::active().bg;
                let nmax = n.max(1) as f32;
                let fade = |p: crate::spatial::Vec3, id: usize, base: egui::Color32, peak: f32| {
                    let height_fade = 0.3 + 0.7 * (1.0 - ((p.y - plane_y) / 6.0).clamp(0.0, 1.0));
                    let age_fade = 0.5 + 0.5 * (id as f32 / nmax); // older ids fainter
                    let a = (peak * refl_amt * height_fade * age_fade) as u8;
                    let d = theme::lerp_col(base, bg, 0.5);
                    egui::Color32::from_rgba_unmultiplied(d.r(), d.g(), d.b(), a)
                };
                for nd in &g.nodes {
                    if let Some(pid) = nd.parent {
                        let a = crate::spatial::Vec3::new(pos[pid].x, plane_y, pos[pid].z);
                        let b = crate::spatial::Vec3::new(pos[nd.id].x, plane_y, pos[nd.id].z);
                        let mut st = m3::with_mat(m3::strand(a, b, 0.03, 4, 5, t, 0.10, edge_col), strand_mat);
                        if name == "Atlantis" { st.color = theme::lerp_col(cord_col, bg, 0.3); st.emissive = 0.32; }
                        else { st.color = fade(pos[nd.id], nd.id, edge_col, 70.0); }
                        graph.push(st);
                    }
                }
                for nd in &g.nodes {
                    let p = pos[nd.id];
                    let gp = crate::spatial::Vec3::new(p.x, plane_y, p.z);
                    let base = if self.active_leaf == Some(nd.id) { theme::CYAN() } else { palette[seg_of[nd.id] % palette.len()] };
                    let mut ghost = m3::with_mat(m3::node_mesh(style, gp, 0.17, nd.id as u32, base), node_mat);
                    if name == "Atlantis" {
                        // hollywood mirror: a softly glowing reflection in dark water, not an opaque dark blob
                        ghost.color = theme::lerp_col(base, bg, 0.35);
                        ghost.emissive = 0.40 * (0.5 + 0.5 * (nd.id as f32 / nmax)); // recent reflections brighter
                    } else {
                        ghost.color = fade(p, nd.id, base, 135.0);
                    }
                    graph.push(ghost);
                }
            }

            // PHYSICAL UI PANEL — theme-aware: each scene casts the message entry in its own idiom (Atlantis
            // pearl/nacre · Canopy backlit leaf-glow · Mistfall carved blue rune · Archive/Reading ink-on-paper).
            if !self.backdrop {
                use crate::spatial::Vec3 as V3;
                // (panel rgb, material, emissive, alpha, text rgb, text-glow multiplier)
                // scale a palette colour into an [u8;3] — used by the derived fallback below to tint the
                // plaque/accent from the active palette rather than a fixed grey.
                let scl = |c: egui::Color32, k: f32| -> [u8; 3] {
                    [(c.r() as f32 * k).clamp(0.0, 255.0) as u8,
                     (c.g() as f32 * k).clamp(0.0, 255.0) as u8,
                     (c.b() as f32 * k).clamp(0.0, 255.0) as u8]
                };
                let (pc, pmat, pemis, palpha, tc, tglow): ([u8; 3], f32, f32, u8, [u8; 3], f32) = match name {
                    "Atlantis"     => ([0xe6, 0xec, 0xf2], crate::texgen::mat::WATER, 0.28, 118, [0x9c, 0xee, 0xff], 1.25),
                    "Canopy"       => ([0x86, 0xb8, 0x52], crate::texgen::mat::MOSS,  0.14, 122, [0xe8, 0xf6, 0x9a], 1.15), // backlit leaf glow
                    "Mistfall"     => ([0x46, 0x60, 0x78], crate::texgen::mat::STONE, 0.08, 158, [0x66, 0xc6, 0xff], 1.0),  // carved blue rune
                    "Archive"      => ([0xe6, 0xd8, 0xbc], crate::texgen::mat::PAPER, 0.04, 188, [0x4a, 0x32, 0x1c], 0.45), // ink on parchment
                    "Reading Room" => ([0xf0, 0xe8, 0xd6], crate::texgen::mat::PAPER, 0.04, 190, [0x2e, 0x24, 0x18], 0.42), // typewriter ink
                    "Mansion Garden" => ([0xf0, 0xe6, 0xcc], crate::texgen::mat::STONE, 0.14, 150, [0xd8, 0xb4, 0x5c], 0.9), // cream marble + gold veins
                    // The three reference worlds match their physical board (see `physkin`): iron-bound
                    // slate with amber text in the cave; dark stained timber with warm cream text in both
                    // Elmbrook districts — one carpentry, so the message plaque and the dock read as cut
                    // from the same boards the context panel hangs on.
                    "Cat Cave"     => ([0x3a, 0x38, 0x36], crate::texgen::mat::STONE, 0.08, 200, [0xf0, 0xc8, 0x84], 1.05), // riveted slate, lantern-lit
                    "Forest Village" => ([0x3e, 0x30, 0x20], crate::texgen::mat::WOOD_WET, 0.06, 205, [0xf2, 0xe0, 0xb8], 1.0), // rain-dark timber
                    "Lavarock Way" => ([0x5a, 0x46, 0x30], crate::texgen::mat::WOOD_SMOOTH, 0.05, 205, [0xf6, 0xea, 0xc8], 0.7), // sun-bleached timber
                    // Every OTHER world derives a coherent idiom from its active palette (material + warm
                    // light + cool accent) instead of the old flat-grey quad, so the message panel and the
                    // mic pearl read as belonging to the scene. Dark worlds glow their accent on a dim tinted
                    // plaque; light worlds ink on a pale surface of their own warm light. This makes the
                    // panel idiom — and thus the mic pearl derived from it — cover all scenes for free.
                    _ => {
                        let pal = theme::active();
                        let (pmat, pemis, palpha) = match pal.material {
                            theme::Material::Paper  => (crate::texgen::mat::PAPER, 0.04, 186),
                            theme::Material::Wood   => (crate::texgen::mat::WOOD,  0.12, 150),
                            theme::Material::Marble => (crate::texgen::mat::STONE, 0.16, 140),
                            _                       => (crate::texgen::mat::STONE, 0.10, 138), // Stone / None
                        };
                        if pal.dark {
                            (scl(pal.light, 0.30), pmat, pemis, palpha, [pal.cyan.r(), pal.cyan.g(), pal.cyan.b()], 1.0)
                        } else {
                            (scl(pal.light, 0.96), pmat, (pemis * 0.6).min(0.06), palpha.max(180), [pal.ink.r(), pal.ink.g(), pal.ink.b()], 0.5)
                        }
                    }
                };
                let (cyw, syw) = (view_cam.yaw.cos(), view_cam.yaw.sin());
                let (cpt, spt) = (view_cam.pitch.cos(), view_cam.pitch.sin());
                let right = V3::new(cyw, 0.0, syw);
                let up = V3::new(syw * spt, cpt, -cyw * spt);
                let toward = V3::new(-(cpt * syw), spt, cpt * cyw);
                let fwd = V3::new(cpt * syw, -spt, -cpt * cyw);
                let tg = view_cam.target;
                let eye = V3::new(tg.x + toward.x * view_cam.dist, tg.y + toward.y * view_cam.dist, tg.z + toward.z * view_cam.dist);
                let (d, drop) = (4.4_f32, -1.45_f32);
                let cen = V3::new(eye.x + fwd.x * d + up.x * drop, eye.y + fwd.y * d + up.y * drop, eye.z + fwd.z * d + up.z * drop);
                // WINDOW-SIZE AWARENESS: canvas dims + the local screen scale (px per world unit at `cen`'s
                // depth), so the physical UI can be ANCHORED to screen regions and SIZED to canvas fractions
                // (via `screen_to_world` / `ppw`) instead of fixed world offsets that drift off small windows.
                let (win_w, win_h) = (rect.width(), rect.height());
                let (ppw_x, ppw_y) = px_per_world(&view_cam, cen, right, up, cx, cy, scale);
                let ppw = ppw_x.min(ppw_y).max(1.0);
                let anchor = |ax: f32, ay: f32| screen_to_world(&view_cam, cen, right, up, cx, cy, scale, rect.left() + ax * win_w, rect.top() + ay * win_h);
                // IN-SCENE PANEL BOX removed by default: the app is moving to a separate message box + text-entry
                // box, so this third boxy plaque is redundant (and some of its layers weren't rendering cleanly).
                // LOGSCOPE_PANEL=1 restores it. The MIC pearl (voice state) + node/voice highlights stay — they're
                // not part of the box.
                // RETIRED as the composer: superseded by the pearl composer further down (reuses
                // pearl_input's proven cell-pool rendering, anchored/scaled the same way as the dial panel
                // and reading box, instead of this block's fixed world offsets — which is exactly what was
                // drifting it off-frame on other aspect ratios). Kept opt-in for comparison only.
                let draw_panel = std::env::var("LOGSCOPE_PANEL").ok().as_deref() == Some("1");
                if draw_panel {
                // DROP SHADOW — a larger, dark, soft quad set further back and offset down-right (as if lit
                // from the upper-left), so the physical UI tablet casts a shadow and reads with real DEPTH
                // instead of floating flat against the scene.
                let scen = V3::new(cen.x + fwd.x * 0.16 + right.x * 0.10 - up.x * 0.10,
                                   cen.y + fwd.y * 0.16 + right.y * 0.10 - up.y * 0.10,
                                   cen.z + fwd.z * 0.16 + right.z * 0.10 - up.z * 0.10);
                let mut shadow = m3::panel(scen, right, up, 4.55, 1.26, egui::Color32::from_rgb(0, 0, 0));
                shadow.mat = crate::texgen::mat::STONE; shadow.emissive = 0.0; m3::set_alpha(&mut shadow, 96);
                graph.push(shadow);
                // bezel frame just BEHIND the plaque (darker shade of the panel, a touch more opaque) so the
                // message entry reads as a physical tablet/plaque rather than a floating quad.
                let bez = if name == "Atlantis" { egui::Color32::from_rgb(0x1c, 0x5c, 0x6a) } else { egui::Color32::from_rgb((pc[0] as f32 * 0.5) as u8, (pc[1] as f32 * 0.5) as u8, (pc[2] as f32 * 0.5) as u8) };
                let fcen = V3::new(cen.x + fwd.x * 0.05, cen.y + fwd.y * 0.05, cen.z + fwd.z * 0.05);
                let mut frame = m3::panel(fcen, right, up, 4.3, 1.06, bez);
                frame.mat = pmat; frame.emissive = if name == "Atlantis" { 0.34 } else { pemis * 0.4 }; m3::set_alpha(&mut frame, (palpha as u16 + 50).min(255) as u8);
                graph.push(frame);
                // a thin glowing accent strip down the left edge, in the scene's text colour — a small UI cue.
                // It flares to the theme ACCENT and pulses when the live voice partial is already an actionable
                // command, so the physical panel itself signals "heard — this will do something".
                let acen = V3::new(cen.x - right.x * 1.92, cen.y - right.y * 1.92, cen.z - right.z * 1.92);
                let (astrip_col, astrip_emis, astrip_alpha) = if voice_actionable {
                    (theme::accent(), 0.9 + 0.5 * (t * 6.0).sin(), 255u8)
                } else {
                    (egui::Color32::from_rgb(tc[0], tc[1], tc[2]), 0.45 + pemis, 224u8)
                };
                let mut accent = m3::panel(acen, right, up, 0.08, 0.66, astrip_col);
                accent.mat = pmat; accent.emissive = astrip_emis; m3::set_alpha(&mut accent, astrip_alpha);
                graph.push(accent);
                let mut panel = m3::panel(cen, right, up, 4.0, 0.82, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                panel.mat = pmat; panel.emissive = pemis;
                m3::set_alpha(&mut panel, palpha);
                graph.push(panel);
                // ACCESSIBILITY / INTERACTION OVERLAY — the message panel is a real 3D object; lay a transparent
                // egui widget over its projected screen rect so the pointer (hover cursor) and screen-reader
                // (AccessKit) both target the physical slab in the scene. Sense::hover so it never steals clicks
                // from the scene behind it. This is the framework for bringing UI controls into the world as
                // interactive 3D assets (project the slab → overlay an egui widget on its rect).
                if let Some(prect) = project_slab_rect(&view_cam, cen, right, up, 4.0, 0.82, cx, cy, scale) {
                    let ov = ui.interact(prect, egui::Id::new("scene_msg_panel"), egui::Sense::click())
                        .on_hover_text("Message panel — click to type here, or speak into the pearl");
                    if ov.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::Text); }
                    // click the physical slab → focus the composer's text input, so the in-scene panel is a real
                    // interactive control (the framework for moving the composer fully into the world).
                    if ov.clicked() {
                        ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("logscope_chat_composer")));
                    }
                }
                let typing = !self.input.trim().is_empty();
                let shown: String = if typing {
                    let nch = self.input.chars().count();
                    self.input.chars().skip(nch.saturating_sub(20)).collect()
                } else { "speak into the pearl".chars().take(20).collect() };
                let mut lit = (if typing { 0.95_f32 } else { 0.34 }) * tglow;
                let mut tcol = egui::Color32::from_rgb(tc[0], tc[1], tc[2]);
                if name == "Frozen Bifrost" { // ASGARD: the panel text reads as a CONSTELLATION of starlight glyph-points
                    tcol = egui::Color32::from_rgb(0xdc, 0xe6, 0xff); lit = lit.max(0.55) + 0.35;
                }
                // LEFT-ALIGN from a fixed left edge so every glyph sits in a STABLE pixel column as the prompt
                // grows (label_cells centres on its origin, which would slide the text around as you type). Font
                // is ~12% smaller than before so longer prompts fit. This is what sells "typing into the panel".
                let tscale = 0.030_f32;
                let nshown = shown.trim().chars().count().min(32) as f32;
                let tw = nshown * 6.0 * tscale;
                let lx = 1.86_f32; // half the text field width (panel half is 2.0)
                let bx = V3::new(cen.x - right.x * lx, cen.y - right.y * lx, cen.z - right.z * lx); // fixed left edge
                let tcen = V3::new(bx.x + right.x * (tw * 0.5) + up.x * 0.06, bx.y + right.y * (tw * 0.5) + up.y * 0.06, bx.z + right.z * (tw * 0.5) + up.z * 0.06);
                for cell in m3::label_cells(tcen, right, up, toward, shown.trim(), tscale, t, tcol, lit, 0.0, tanim, m3::SceneLight::Uniform, 0) {
                    graph.push(cell);
                }
                // blinking caret at the end of the entry (the input "cursor"), so the selection point is visible.
                if typing && (t * 2.0).sin() > 0.0 {
                    let cx = bx.x + right.x * (tw + tscale * 2.0);
                    let cyv = bx.y + right.y * (tw + tscale * 2.0) + up.y * 0.06;
                    let cz = bx.z + right.z * (tw + tscale * 2.0) + up.z * 0.06;
                    for ky in 0..7 {
                        let py = (3.0 - ky as f32) * tscale;
                        let mut cm = m3::uv_sphere(V3::new(cx + up.x * py, cyv + up.y * py - up.y * 0.0, cz + up.z * py), tscale * 0.5, 4, 3, tcol);
                        cm.emissive = 1.4 * tglow; graph.push(cm);
                    }
                }
                } // end draw_panel (in-scene panel box)
                // MIC PEARL — a small marble-sheen orb beside the message panel, showing live voice state at a
                // glance. Deliberately a DIFFERENT physical object from the flat resonance-cell text (a glossy
                // stone/pearl bead rather than glyph cells), but its colour is DERIVED from the same per-scene
                // (pc, tc, tglow, pmat) table above — so every scene's mic pearl is a free, coherent derivative
                // of its message-panel idiom rather than a bespoke style per world.
                //   asleep (mic off)   — dim grey mote, barely breathing.
                //   armed (mic on)     — a warm marble shell, slow breathing glow in the scene's accent colour.
                //   active (hearing an utterance) — brighter + faster pulse + two orbiting sparkle motes, so
                //     "it's catching your voice" reads even before whisper finalizes the utterance.
                // armed/active state: real self.voice, or LOGSCOPE_MIC_PEARL_DEBUG=armed|active for headless
                // render verification (mic-pearl-view bin) — the `voice` feature can't build without a real
                // ALSA/whisper.cpp toolchain, so this is how the pearl's states get visually checked per scene.
                let debug = std::env::var("LOGSCOPE_MIC_PEARL_DEBUG").ok();
                let armed = self.voice.is_some() || matches!(debug.as_deref(), Some("armed") | Some("active"));
                let active = self.voice.as_ref().map(|v| !v.partial().is_empty()).unwrap_or(false) || debug.as_deref() == Some("active");
                let mcen = V3::new(cen.x + right.x * 2.22 + fwd.x * 0.06, cen.y + up.y * 0.02 + fwd.y * 0.06, cen.z + right.z * 2.22 + fwd.z * 0.06);
                let mshape = mic_shape(name, theme::active().material);
                // PHYSICAL MIC RESPONSE — replaces a fixed-rate sine (identical whether the room is
                // silent or someone is shouting) with `micresponse`, driven by the capture's real
                // rolling RMS. The RESPONDER follows the shape's own substance: a leaf has mass and
                // keeps moving after a word ends, a gem/stone/rune-adjacent body flashes on the onset,
                // a bead ripples with the rate of change, and the plain pearl reads as a level meter.
                let mic_level = self.voice.as_ref().map(|v| v.level()).unwrap_or(0.0);
                let dt = ui.input(|i| i.stable_dt).min(0.1);
                let mic_drivers = crate::micresponse::drivers((mic_level * 12.0).min(1.0), self.mic_prev_level, dt);
                self.mic_prev_level = mic_drivers.level;
                let mic_responder = match mshape {
                    MicShape::Leaf => crate::micresponse::Responder::Leaf,
                    MicShape::Gem | MicShape::Stone => crate::micresponse::Responder::Rune,
                    MicShape::Bead => crate::micresponse::Responder::Bead,
                    MicShape::Pearl => crate::micresponse::Responder::Pearl,
                };
                self.mic_motion.step(mic_responder, &mic_drivers, dt);
                if self.mic_motion.value > crate::micresponse::REST_EPS { ui.ctx().request_repaint(); }
                // debug/back-compat: LOGSCOPE_MIC_PEARL_DEBUG or no live capture (voice feature off,
                // mic asleep) falls back to the old canned breathing so the shape stays visibly alive
                // in headless/verification renders that have no real audio source.
                let has_signal = self.voice.is_some() && std::env::var("LOGSCOPE_MIC_PEARL_DEBUG").is_err();
                let breathe = if has_signal { self.mic_motion.value.clamp(0.0, 1.0) }
                    else if active { (t * 3.4).sin() * 0.5 + 0.5 } else { (t * 1.1).sin() * 0.5 + 0.5 };
                // LOGSCOPE_MIC_PEARL_SCALE (default 1.0) enlarges the pearl for per-scene SHAPE verification —
                // the ship bead is small (its shape reads as a silhouette/material cue), so this debug hook
                // (paired with LOGSCOPE_MIC_PEARL_DEBUG) lets the headless harness show the gem/pearl/stone/bead
                // geometry clearly across scenes without affecting the shipped size.
                let mscale = std::env::var("LOGSCOPE_MIC_PEARL_SCALE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0).clamp(0.3, 12.0);
                let r = (0.12 + (if active { 0.045 } else { 0.018 }) * breathe) * mscale;
                let shell = if voice_actionable {
                    theme::accent() // an actionable command is already recognized in the live partial → flash accent
                } else if armed {
                    egui::Color32::from_rgb((pc[0] as u16 + 18).min(255) as u8, (pc[1] as u16 + 14).min(255) as u8, (pc[2] as u16 + 6).min(255) as u8)
                } else { egui::Color32::from_rgb(0x6a, 0x6a, 0x70) }; // asleep: dim grey mote, no scene tint
                // SHAPE is the scene's physical substance (a second derivative beyond colour) — it holds
                // across all states, so an asleep gem still reads as a gem. A stable per-scene seed varies the
                // gem facets / stone lumps between worlds. Colour/material/emissive still track the mic state.
                let mseed = name.bytes().fold(0x811C_9DC5u32, |a, b| (a ^ b as u32).wrapping_mul(0x0100_0193));
                let mut orb = match mshape {
                    MicShape::Gem   => m3::cracked_pearl(mcen, r, mseed, shell), // faceted iridescent crystal
                    MicShape::Stone => m3::rock(mcen, r, mseed, shell),          // weathered / carved stone bead
                    MicShape::Bead  => m3::uv_sphere(mcen, r, 7, 5, shell),      // chunky low-poly turned wood bead
                    MicShape::Leaf  => m3::leaf_single(mcen, r * 1.35, mseed, shell), // backlit foliage leaf blade
                    MicShape::Pearl => m3::uv_sphere(mcen, r, 10, 8, shell),     // smooth pearl / paper wafer
                };
                orb.mat = if !armed { crate::texgen::mat::STONE }
                          else if mshape == MicShape::Leaf { crate::texgen::mat::MOSS } // leaf reads as translucent foliage
                          else { pmat };
                orb.emissive = if voice_actionable { (0.9 + 0.7 * breathe) * tglow } else if !armed { 0.06 } else if active { (0.55 + 0.85 * breathe) * tglow } else { (0.18 + 0.22 * breathe) * tglow };
                graph.push(orb);
                // SCENE-RELEVANT SETTING — the pearl sits IN something that belongs to the world (shell / gem
                // cradle / rune plinth / wooden dish / dew stem), so the voice indicator reads as a placed
                // asset, not a primitive floating in space. Dim + matte so the glowing orb still leads.
                let shape_code = match mshape { MicShape::Pearl => 0, MicShape::Gem => 1, MicShape::Stone => 2, MicShape::Bead => 3, MicShape::Leaf => 4 };
                for m in crate::scene3d::mic_setting(name, shape_code, mcen, r, right, up, fwd, mseed, egui::Color32::from_rgb(pc[0], pc[1], pc[2])) {
                    graph.push(m);
                }
                // ACCESSIBILITY OVERLAY on the mic object — the second in-scene UI control exposed to pointer +
                // screen-reader, labelled by voice state. Now CLICKABLE: pressing the physical pearl arms/
                // disarms the mic, the same action as the 2D "🎙 Mic" toolbar button (same VoiceSession::start
                // / drop-to-stop), so the object is a real control rather than a lit indicator you can only
                // look at. Errors surface the same way the toolbar button does (self.voice_err).
                if let Some(mrect) = project_slab_rect(&view_cam, mcen, right, up, r * 2.4, r * 2.4, cx, cy, scale) {
                    let label = if active { "Voice mic — listening (click to stop)" }
                        else if armed { "Voice mic — armed (click to stop)" }
                        else { "Voice mic — off (click to start)" };
                    let mo = ui.interact(mrect, egui::Id::new("scene_mic_object"), egui::Sense::click()).on_hover_text(label);
                    if mo.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                    if mo.clicked() {
                        if self.voice.is_some() { self.voice = None; }
                        else {
                            match crate::voice_session::VoiceSession::start() {
                                Ok(s) => { self.voice = Some(s); self.voice_err = None; ui.ctx().request_repaint(); }
                                Err(e) => { self.voice_err = Some(format!("voice: {e}")); }
                            }
                        }
                    }
                }
                if active {
                    // two tiny orbiting sparkle motes in the scene's accent colour — the pearl actively
                    // "catching" your voice, distinct from the steady breathing glow of just being armed.
                    for i in 0..2u32 {
                        let ang = t * 2.2 + i as f32 * std::f32::consts::PI;
                        let orad = r * 2.1;
                        let ox = mcen.x + right.x * ang.cos() * orad + up.x * ang.sin() * orad * 0.6;
                        let oy = mcen.y + right.y * ang.cos() * orad + up.y * ang.sin() * orad * 0.6;
                        let oz = mcen.z + right.z * ang.cos() * orad + up.z * ang.sin() * orad * 0.6;
                        let mut mote = m3::uv_sphere(V3::new(ox, oy, oz), r * 0.22, 5, 4, egui::Color32::from_rgb(tc[0], tc[1], tc[2]));
                        mote.emissive = 1.6 * tglow;
                        graph.push(mote);
                    }
                }

                // ── PHYSICAL CONTROL DOCK ────────────────────────────────────────────────────────
                // Overview / Tour / Freecam as in-scene physical BUTTONS (real 3D objects you press),
                // driven by the tested `physui` FSM. `dock_arc` lays them on a shallow arc below the
                // pearl; `project_slab_rect` hit-tests each; `ControlAnim` eases the press/hover/lit
                // feel; and each actuation drives the SAME action as the 2D toolbar (which stays — so
                // nothing that works today breaks). LOGSCOPE_PHYS_DOCK=0 disables it (A/B + safety valve).
                if !world_only
                    && std::env::var("LOGSCOPE_PHYS_DOCK").map(|v| v != "0").unwrap_or(true)
                {
                    use crate::physui::Pointer;
                    // Keep the physical toggles in step with state changed elsewhere (2D button, voice
                    // command) BEFORE reading input, so a press starts from the true lit state.
                    self.phys.set_active(DOCK_TOUR, self.fly_auto.is_some());
                    self.phys.set_active(DOCK_FREECAM, self.freecam);

                    // dt for the eased button feel (clamped so a long stall doesn't jump the ease).
                    let now = std::time::Instant::now();
                    let dt = self.phys_t.map(|p| (now - p).as_secs_f32()).unwrap_or(1.0 / 60.0).clamp(0.0, 0.1);
                    self.phys_t = Some(now);

                    // Lay the dock on a shallow arc just below the message pearl, curving toward the
                    // viewer; billboarded slabs (right/up basis) so each button faces the camera. The
                    // small downward offset keeps it a ledge beneath the pearl without dropping off the
                    // bottom of the canvas (the panel origin already sits low-centre).
                    let dock_radius = (0.42 * win_w / ppw / 1.36).clamp(0.55, 1.7); // arc spans ~42% of width
                    let ds = dock_radius / 0.95; // scale buttons with the arc
                    // BOTTOM-EDGE FIT. The old constant 0.87 placed the arc's CENTRE, so the lower
                    // half of each slab hung below it — and since the slabs scale with window width
                    // (ds), a wide window pushed that overhang past the canvas edge and the bottom
                    // row was clipped. Measure the viewport's height in world units by sampling the
                    // same anchor mapping at top and bottom, then let physui place the centre so the
                    // slab's lower edge lands a margin above the edge. Tested in physui, which is
                    // egui-free and runs standalone.
                    let vspan = { let (t, b) = (anchor(0.5, 0.0), anchor(0.5, 1.0));
                        ((t.x - b.x).powi(2) + (t.y - b.y).powi(2) + (t.z - b.z).powi(2)).sqrt() };
                    let dock_v = crate::physui::dock_center_v(0.26_f32 * ds, vspan, 0.045);
                    // Adjacent buttons' slabs were touching/fusing (screenshot comparison after the off-
                    // screen fix above): at spread=1.5 rad over 5 buttons, the arc's chord spacing between
                    // centres works out to ~0.373·radius while bw was ~0.305·radius — only an 18% gap,
                    // which read as fused given the slabs' own drop-shadow/glint bleed. Narrowed the slabs
                    // rather than widening the arc's spread, since widening also grows the arc's total
                    // horizontal footprint and risks the same off-screen-edge failure just fixed above.
                    let (bw, bh) = (0.29_f32 * ds * 0.76, 0.26_f32 * ds); // button slab size (world units)
                    // HUD PACKING: register the dock's footprint FIRST (it renders first, so it wins any
                    // conflict) — see the `hud_placed` declaration above for why this exists at all.
                    let dock_c = hud_place(&mut hud_placed, rect, &view_cam, right, up, cx, cy, scale,
                        anchor(0.5, dock_v), dock_radius * 1.9 + bw, bh * 1.4 + dock_radius * 2.2, 6.0);
                    let pos = crate::physui::dock_arc(
                        DOCK_IDS.len(),
                        [dock_c.x, dock_c.y, dock_c.z],
                        [right.x, right.y, right.z],
                        [toward.x, toward.y, toward.z], // arc bulges toward the user
                        dock_radius, 1.5,
                    );
                    // Prunable churn (matches the 2D "Prune churn (N)" button): superseded turns not yet pruned.
                    let sup = g.superseded();
                    let prunable = sup.iter().filter(|id| !self.pruned.contains(id)).count();

                    // Project each button to a screen rect (hit-test target + a11y overlay bounds).
                    let mut rects: Vec<(usize, egui::Rect)> = Vec::with_capacity(DOCK_IDS.len());
                    for (i, &id) in DOCK_IDS.iter().enumerate() {
                        let bc = V3::new(pos[i][0], pos[i][1], pos[i][2]);
                        if let Some(rct) = project_slab_rect(&view_cam, bc, right, up, bw * 1.2, bh * 1.5, cx, cy, scale) {
                            rects.push((id, rct));
                        }
                    }

                    // Drive the physui FSM from the raw pointer: which control is under the pointer, and
                    // did the primary button go down / up this frame. Actuation happens on a release over
                    // the SAME control the press started on (drag-off cancels) — the FSM enforces that;
                    // here we just feed it. Raw pointer state, so it's independent of egui click routing.
                    let ptr = ui.input(|i| i.pointer.interact_pos());
                    let over = ptr.and_then(|pp| rects.iter().find(|(_, r)| r.contains(pp)).map(|(id, _)| *id));
                    let (down, up_ev) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
                    let ev = if down { self.phys.on_pointer(Pointer::Down, over) }
                             else if up_ev { self.phys.on_pointer(Pointer::Up, over) }
                             else { self.phys.on_pointer(Pointer::Move, over) };

                    // Route an actuation to the SAME field mutations as the 2D toolbar buttons.
                    if let Some(act) = ev.and_then(dock_action) {
                        // kick the actuation flash on the control that fired (so the press visibly *does* something)
                        let fired = match ev { Some(crate::physui::ControlEvent::Fired(i)) | Some(crate::physui::ControlEvent::Toggled(i, _)) => Some(i), _ => None };
                        if let Some(fid) = fired { if let Some(ix) = DOCK_IDS.iter().position(|&d| d == fid) { self.phys_anim[ix].kick_flash(); } }
                        match act {
                            DockAction::Overview => { self.spatial_auto = true; self.spatial_zoomed = false; self.walk_focus = None; }
                            DockAction::Tour(on) => {
                                self.fly_auto = if on { Some(1.0) } else { None };
                                if on { self.freecam = false; } else { self.spatial_auto = true; self.spatial_zoomed = false; }
                            }
                            DockAction::Freecam(on) => {
                                self.freecam = on;
                                if on { self.fly_auto = None; self.spatial_auto = false; self.spatial_zoomed = true; }
                                else { self.spatial_auto = true; self.spatial_zoomed = false; }
                            }
                            DockAction::Prune => { for id in &sup { self.pruned.insert(*id); } } // no-op when nothing superseded
                            DockAction::Unprune => { self.pruned.clear(); } // restore every pruned turn
                        }
                        ui.ctx().request_repaint();
                    }
                    // Re-sync the toggles so THIS frame's anim reads the just-applied lit state.
                    self.phys.set_active(DOCK_TOUR, self.fly_auto.is_some());
                    self.phys.set_active(DOCK_FREECAM, self.freecam);

                    // Draw each button eased by its ControlAnim: hover brightens, press pushes the slab
                    // IN (toward −toward, away from the eye), active lights it. Each face carries a short
                    // glowing LABEL in its control colour (cyan Overview · amber Tour · green Freecam ·
                    // rose Prune), so the dock reads without hovering. Prune dims when there's nothing to
                    // prune. State-aware labels: TOUR↔STOP, FREE↔EXIT.
                    let pip = [
                        egui::Color32::from_rgb(0x7a, 0xe0, 0xff), // Overview — cyan
                        egui::Color32::from_rgb(0xff, 0xc2, 0x6a), // Tour — amber
                        egui::Color32::from_rgb(0x8e, 0xe6, 0x9a), // Freecam — green
                        egui::Color32::from_rgb(0xff, 0x92, 0x9a), // Prune — rose
                        egui::Color32::from_rgb(0xc9, 0xa8, 0xff), // Unprune — lavender
                    ];
                    let face = [
                        "VIEW",
                        if self.fly_auto.is_some() { "STOP" } else { "TOUR" },
                        if self.freecam { "EXIT" } else { "FREE" },
                        "PRUNE",
                        "UNDO",
                    ];
                    let mut animating = false;
                    // OVERLAY BIAS: same mechanism as the reading plaque / PARAMS panel (pull toward camera in
                    // world space so the fragment wins the depth test) — this dock never had it either, so a
                    // scene prop nearer the camera than the dock's natural depth could render through it.
                    let ov_dock = 0.9_f32;
                    for (i, &id) in DOCK_IDS.iter().enumerate() {
                        let vis = self.phys.visual(id);
                        self.phys_anim[i].tick(vis, dt);
                        // DEBUG (headless verification of the tactile visuals): force control #i into a held
                        // press + flash so a render shows the depression, flash ring, and label pop.
                        if std::env::var("LOGSCOPE_PHYS_FORCE").ok().and_then(|s| s.trim().parse::<usize>().ok()) == Some(i) {
                            self.phys_anim[i].press = 1.0; self.phys_anim[i].hover = 1.0;
                            self.phys_anim[i].flash = self.phys_anim[i].flash.max(0.7);
                        }
                        let a = self.phys_anim[i];
                        if a.flash > 0.002 || (a.press.abs() > 0.002 && !vis.pressed) || a.hover > 0.002 { animating = true; }
                        // Prune enabled only when there's churn to drop; Unprune only when something is pruned.
                        let enabled = match id {
                            DOCK_PRUNE => prunable > 0,
                            DOCK_UNPRUNE => !self.pruned.is_empty(),
                            _ => true,
                        };
                        // springy press: pushes IN when pressed, pops slightly PROUD (press<0) on release.
                        // Hover ALSO lifts it a touch proud — before now hover only brightened the slab
                        // (emissive), giving no depth cue at all for the most common interaction (mouse
                        // over, no click yet). Same push mechanism, just a smaller magnitude than the
                        // press-release pop so the two states stay visually distinct.
                        let push = a.press * 0.07 - a.hover * 0.03;
                        let bc = V3::new(pos[i][0] - toward.x * push, pos[i][1] - toward.y * push, pos[i][2] - toward.z * push);
                        let base = egui::Color32::from_rgb(pc[0], pc[1], pc[2]);
                        let mut btn = m3::panel(bc, right, up, bw, bh, base);
                        btn.mat = pmat; btn.overlay = ov_dock;
                        let dim = if enabled { 1.0 } else { 0.4 };
                        // emissive: base + hover + active + a bright ACTUATION FLASH burst
                        btn.emissive = (0.12 + 0.55 * a.hover + 0.85 * a.active + 1.5 * a.flash) * tglow * dim * sheen;
                        m3::set_alpha(&mut btn, if enabled { 220 } else { 150 });
                        graph.push(btn); slab_glint(&mut graph, bc, right, up, toward, bw * 0.5, bh * 0.5, sun, tglow);
                        // actuation flash RING — a thin frame that flares just outside the slab and fades, so a
                        // press reads as a real event even at a glance.
                        if a.flash > 0.02 {
                            let g = 1.0 + 0.28 * a.flash; // expands as it fires
                            let mut ring = m3::panel(bc, right, up, bw * g + 0.06, bh * g + 0.06, pip[i]);
                            ring.mat = pmat; ring.overlay = ov_dock + 0.05;
                            ring.emissive = 2.2 * a.flash * tglow.max(0.6);
                            m3::set_alpha(&mut ring, (150.0 * a.flash) as u8);
                            graph.push(ring);
                        }
                        // glowing face label, floated just proud of the slab. A change/press makes it POP
                        // (scale + brightness) AND slide into place (scroll) via the flash, so text visibly
                        // reacts — a TOUR→STOP swap reads as motion, not an instant flip.
                        let lbl = face[i];
                        let lscale = (bw * 0.86 / (lbl.len() as f32 * 6.0)).min(0.017) * (1.0 + 0.14 * a.flash);
                        let lcen = V3::new(bc.x - toward.x * 0.30, bc.y - toward.y * 0.30, bc.z - toward.z * 0.30);
                        let llit = if enabled { (0.55 + 0.5 * a.hover + 0.7 * a.active + 0.8 * a.flash).min(1.6) } else { 0.18 };
                        for mut cell in m3::label_cells(lcen, right, up, toward, lbl, lscale, t, pip[i], llit, a.flash * 0.06, tanim, m3::SceneLight::Uniform, 0) {
                            cell.overlay = ov_dock + 0.15;
                            graph.push(cell);
                        }
                    }
                    if animating { ui.ctx().request_repaint(); }

                    // Accessibility / interaction overlay: a transparent egui widget over each button's
                    // projected rect gives the pointer (hand cursor) and screen-reader (AccessKit) a real
                    // target, and — being added after the scene canvas — keeps a button press from also
                    // orbiting the camera. Labels track the toggle state.
                    for &id in DOCK_IDS.iter() {
                        if let Some((_, rct)) = rects.iter().find(|(rid, _)| *rid == id) {
                            let label = match id {
                                DOCK_OVERVIEW => "Overview — recenter and fit the whole graph".to_string(),
                                DOCK_TOUR => if self.fly_auto.is_some() { "Stop tour".into() } else { "Tour — fly through the palace".into() },
                                DOCK_FREECAM => if self.freecam { "Exit freecam".into() } else { "Freecam — free-flight camera".into() },
                                DOCK_PRUNE => if prunable > 0 { format!("Prune churn ({prunable}) — drop superseded turns to free context") } else { "Prune churn — nothing to prune".into() },
                                DOCK_UNPRUNE => if !self.pruned.is_empty() { format!("Unprune — restore all {} pruned turns to context", self.pruned.len()) } else { "Unprune — nothing pruned".into() },
                                _ => String::new(),
                            };
                            let resp = ui.interact(*rct, egui::Id::new(("scene_phys_ctrl", id)), egui::Sense::click())
                                .on_hover_text(label);
                            if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                        }
                    }

                    // ── PHYSICAL SETTINGS PANEL — the Context panel's sampling sliders as physical DIALS ──
                    // A plaque on the left of the scene, one draggable dial per self.ctx param (temperature, ctx
                    // window, reply budget, repeat penalty, min-p). Two-way bound: the handle tracks the live value
                    // (so 2D edits reflect) and dragging writes back to self.ctx. (The old time dial is gone —
                    // the scene tracks system time.) LOGSCOPE_PHYS_PARAMS=0 hides it.
                    if std::env::var("LOGSCOPE_PHYS_PARAMS").map(|v| v != "0").unwrap_or(true) {
                        let panel_c = anchor(0.15, 0.60); // lower-left corner, adapts to window size
                        let plaque_w = (0.24 * win_w / ppw).clamp(0.9, 2.6);
                        let ps = plaque_w / 1.42; // scale the tuned layout by the window-derived plaque width
                        // OVERLAY BIAS: physical UI floating in front of the world — same mechanism + value
                        // (0.9) as the reading plaque. This panel never had it at all (only the reading plaque
                        // did), so any scene prop that happened to sit nearer the camera rendered THROUGH it —
                        // confirmed by screenshot comparison: a stone slab cut straight across REP/MINP/
                        // BACKEND/PERSONA at a camera angle where the slab's natural depth was closer than the
                        // panel's. Applied to every mesh this block pushes, same as the reading plaque does.
                        let ov_p = 0.9_f32;
                        // BACKING SIZE: content is the title + one row per PARAM_DIAL + one row per selector
                        // (BACKEND/PERSONA/MODEL/ROLE — 4 slots reserved even when MODEL is hidden, since the
                        // row loop below skips PUSHING that row's content without renumbering the rest, so the
                        // vertical space stays fixed either way). Rows are spaced 0.26*ps apart, row i's centre
                        // at (0.42 - i*0.26)*ps above `panel_c`. A plaque sized for a handful of dials alone
                        // left the selector rows spilling out past its own bottom edge into whatever rendered
                        // behind it (the PARAMS/PERSONA overlap from screenshot comparison — this is content
                        // overflowing its own container, not two separate panels colliding, so `hud_pack`
                        // doesn't apply here). Sized/centred from the actual row count instead of a fixed
                        // constant that only ever matched however many rows existed when it was last tuned.
                        let n_rows = PARAM_DIALS.len() + 4;
                        let content_top = 0.42 * ps + 0.24 * ps;                          // clearance above row 0 for the title
                        let content_bot = (0.42 - (n_rows as f32 - 1.0) * 0.26) * ps - 0.13 * ps; // clearance below the last row
                        let plaque_h = (content_top - content_bot) + 0.10 * ps;
                        let plaque_cy = (content_top + content_bot) * 0.5;
                        let back_v = |o: f32, dy: f32| V3::new(panel_c.x - toward.x * o + up.x * dy, panel_c.y - toward.y * o + up.y * dy, panel_c.z - toward.z * o + up.z * dy);
                        // backing plaque (set back so the sliders sit proud of it)
                        // BACKING WIDTH: 1.42*ps was tuned for the dial rows' short numeric values ("0.35",
                        // "4096") — the selector rows below them show longer strings (backend/persona names)
                        // that ran straight off the right edge into the scene behind the panel. Widened to
                        // give those values roughly a 10-character budget before they hit the edge; anything
                        // longer still gets a hard ellipsis-truncate below rather than clipping unbounded.
                        let bg_w = 2.10 * ps;
                        let mut plaque = m3::panel(back_v(-0.06 * ps, plaque_cy), right, up, bg_w, plaque_h, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                        plaque.mat = pmat; plaque.emissive = 0.07 * tglow * sheen; m3::set_alpha(&mut plaque, 150); plaque.overlay = ov_p;
                        graph.push(plaque); slab_glint(&mut graph, back_v(-0.06 * ps, plaque_cy), right, up, toward, 0.71 * ps, plaque_h * 0.5, sun, tglow);
                        let title_c = V3::new(panel_c.x - toward.x * 0.30 + up.x * 0.66 * ps, panel_c.y - toward.y * 0.30 + up.y * 0.66 * ps, panel_c.z - toward.z * 0.30 + up.z * 0.66 * ps);
                        // LEGIBILITY FLOOR. `label_cells` builds glyphs from 6x7 grids of
                        // quads; at 0.010 * ps a cell projected to well under a screen pixel
                        // and the letters collapsed into each other — "PARAMS" rendered as
                        // "TAREHE" in a screenshot, and every dial label with it. The floor is
                        // the 3D counterpart of the 2D text floor: a label may be bigger than
                        // legible, never smaller.
                        let (tppw, _) = px_per_world(&view_cam, title_c, right, up, cx, cy, scale);
                        let title_cell = (0.010 * ps).max(m3::legible_cell_scale(tppw));
                        for mut cell in m3::label_cells(title_c, right, up, toward, "PARAMS", title_cell, t, egui::Color32::from_rgb(232, 238, 252), 0.7, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = ov_p + 0.15; graph.push(cell); }

                        let (p_down, p_pressed) = ui.input(|i| (i.pointer.primary_down(), i.pointer.primary_pressed()));
                        let p_pos = ui.input(|i| i.pointer.interact_pos());
                        if !p_down { self.slider_drag = None; }
                        let (tw, th) = (0.64_f32 * ps, 0.038_f32 * ps);
                        for (row, &(id, label, lo, hi, isint)) in PARAM_DIALS.iter().enumerate() {
                            let off = (0.42 - row as f32 * 0.26) * ps;
                            let rc = V3::new(panel_c.x + up.x * off, panel_c.y + up.y * off, panel_c.z + up.z * off);
                            let track_rect = project_slab_rect(&view_cam, rc, right, up, tw, th * 3.2, cx, cy, scale);
                            if let Some(tr) = track_rect {
                                if p_pressed && p_pos.map_or(false, |p| tr.contains(p)) { self.slider_drag = p_pos.map(|p| (id, p.x)); }
                            }
                            let dragging_this = matches!(self.slider_drag, Some((d, _)) if d == id);
                            if dragging_this {
                                if let (Some((_, last_x)), Some(p), Some(tr)) = (self.slider_drag, p_pos, track_rect) {
                                    let dx = p.x - last_x;
                                    if dx.abs() > 0.01 {
                                        let frac = dx / tr.width().max(1.0);
                                        if let Some(crate::physui::ControlEvent::Adjusted(_, v)) = self.phys.drag(id, frac) { ctx_set(&mut self.ctx, id, v); }
                                        self.slider_drag = Some((id, p.x));
                                        ui.ctx().request_repaint();
                                    }
                                }
                            } else {
                                // handle tracks the live value when not being dragged
                                self.phys.set_value(id, ctx_get(&self.ctx, id));
                            }
                            let val = self.phys.value_of(id);
                            let frac = ((val - lo) / (hi - lo)).clamp(0.0, 1.0);
                            let mut trk = m3::panel(rc, right, up, tw, th, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                            trk.mat = pmat; trk.emissive = 0.12 * tglow * sheen; m3::set_alpha(&mut trk, 185); trk.overlay = ov_p;
                            graph.push(trk);
                            let hx = (frac - 0.5) * tw;
                            let hc = V3::new(rc.x + right.x * hx, rc.y + right.y * hx, rc.z + right.z * hx);
                            let mut handle = m3::uv_sphere(hc, th * 1.7, 8, 6, egui::Color32::from_rgb(0x8a, 0xe0, 0xff));
                            handle.mat = pmat; handle.emissive = (0.5 + if dragging_this { 0.9 } else { 0.35 }) * tglow.max(0.6); handle.overlay = ov_p + 0.1;
                            graph.push(handle);
                            // label to the left, value to the right (floated proud of the track)
                            let lx = -(tw * 0.5 + 0.20 * ps);
                            let lcen = V3::new(rc.x - toward.x * 0.30 + right.x * lx + up.x * 0.11 * ps, rc.y - toward.y * 0.30 + right.y * lx + up.y * 0.11 * ps, rc.z - toward.z * 0.30 + right.z * lx + up.z * 0.11 * ps);
                            for mut cell in m3::label_cells(lcen, right, up, toward, label, 0.010 * ps, t, egui::Color32::from_rgb(206, 214, 234), 0.6, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = ov_p + 0.15; graph.push(cell); }
                            let vtxt = if isint { format!("{}", val.round() as i32) } else { format!("{val:.2}") };
                            let vx = tw * 0.5 + 0.10 * ps;
                            let vcen = V3::new(rc.x - toward.x * 0.30 + right.x * vx + up.x * 0.11 * ps, rc.y - toward.y * 0.30 + right.y * vx + up.y * 0.11 * ps, rc.z - toward.z * 0.30 + right.z * vx + up.z * 0.11 * ps);
                            for mut cell in m3::label_cells(vcen, right, up, toward, &vtxt, 0.010 * ps, t, egui::Color32::from_rgb(0x8a, 0xe0, 0xff), 0.72, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = ov_p + 0.15; graph.push(cell); }
                            if let Some(tr) = track_rect {
                                let hint = format!("{label}: {vtxt} — drag to change");
                                let resp = ui.interact(tr, egui::Id::new(("scene_phys_param", id)), egui::Sense::click_and_drag()).on_hover_text(hint);
                                if resp.hovered() || dragging_this { ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal); }
                            }
                        }

                        // ── SELECTOR ROWS — backend / persona / model, the popout's remaining controls,
                        // as physical cycle-plaques below the dials. A click advances to the next option
                        // (physui::ControlKind::Selector, wrapping); the label shows the CURRENT option so
                        // the whole thing reads as a labeled dial rather than a dropdown pretending to be 3D.
                        self.phys.set_index(BACKEND_SEL, if self.backend == BackendKind::ALL[0] { 0 } else { 1 });
                        let persona_idx = PERSONAS.iter().position(|(_, _, body)| self.context.trim() == body.trim()).unwrap_or(usize::MAX);
                        if persona_idx != usize::MAX { self.phys.set_index(PERSONA_SEL, persona_idx); }
                        if let Some(m) = model.as_deref_mut() {
                            self.phys.set_selector_count(MODEL_SEL, m.items.len());
                            self.phys.set_index(MODEL_SEL, m.selected);
                        }
                        // ROLE row: which job the selected model is doing. Uses the Liaison/Disciple/
                        // Thinker vocabulary rather than draft/target, because "target" reads as a
                        // destination when it is the model doing the actual thinking, and a reader has
                        // to be able to tell that a Disciple's output is PROVISIONAL.
                        self.phys.set_selector_count(ROLE_SEL, 3);
                        let sel_rows: &[(usize, &str)] = &[(BACKEND_SEL, "BACKEND"), (PERSONA_SEL, "PERSONA"),
                                                           (MODEL_SEL, "MODEL"), (ROLE_SEL, "ROLE")];
                        for (row, &(id, label)) in sel_rows.iter().enumerate() {
                            if id == MODEL_SEL && model.is_none() { continue; } // no model context (headless/backdrop scene)
                            let off = (0.42 - (PARAM_DIALS.len() + row) as f32 * 0.26) * ps;
                            let rc = V3::new(panel_c.x + up.x * off, panel_c.y + up.y * off, panel_c.z + up.z * off);
                            let track_rect = project_slab_rect(&view_cam, rc, right, up, tw, th * 3.2, cx, cy, scale);
                            let cur = match id {
                                BACKEND_SEL => BackendKind::ALL[self.phys.index_of(id).min(1)].label().to_string(),
                                PERSONA_SEL => PERSONAS.get(self.phys.index_of(id)).map(|(_, title, _)| title.to_string()).unwrap_or_default(),
                                MODEL_SEL => model.as_deref().and_then(|m| m.items.get(m.selected)).map(|it| it.name().to_string()).unwrap_or_else(|| "—".into()),
                                ROLE_SEL => {
                                    use logscope::modelroles::Role;
                                    [Role::Liaison, Role::Disciple, Role::Thinker]
                                        .get(self.phys.index_of(ROLE_SEL)).copied()
                                        .unwrap_or(Role::Thinker).label().to_string()
                                }
                                _ => String::new(),
                            };
                            let mut trk = m3::panel(rc, right, up, tw, th, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                            trk.mat = pmat; trk.emissive = 0.12 * tglow * sheen; m3::set_alpha(&mut trk, 185); trk.overlay = ov_p;
                            graph.push(trk);
                            let lx = -(tw * 0.5 + 0.20 * ps);
                            let lcen = V3::new(rc.x - toward.x * 0.30 + right.x * lx + up.x * 0.11 * ps, rc.y - toward.y * 0.30 + right.y * lx + up.y * 0.11 * ps, rc.z - toward.z * 0.30 + right.z * lx + up.z * 0.11 * ps);
                            for mut cell in m3::label_cells(lcen, right, up, toward, label, 0.010 * ps, t, egui::Color32::from_rgb(206, 214, 234), 0.6, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = ov_p + 0.15; graph.push(cell); }
                            // VALUE position was missing its right-axis offset entirely (`rc` with no `right * vx`
                            // term), landing it at the row's centre — right where the label's own text also
                            // reaches, which is the actual mechanism behind the BACKEND/PERSONA overlap found by
                            // screenshot comparison (the earlier fix in this same panel corrected the backing
                            // plaque's SIZE; this is the separate bug that fix didn't touch). Mirrors the param
                            // dials' own value placement (`vx = tw*0.5 + 0.10*ps`) a few lines above this loop.
                            let vx = tw * 0.5 + 0.10 * ps;
                            let vcen = V3::new(rc.x - toward.x * 0.30 + right.x * vx + up.x * 0.11 * ps, rc.y - toward.y * 0.30 + right.y * vx + up.y * 0.11 * ps, rc.z - toward.z * 0.30 + right.z * vx + up.z * 0.11 * ps);
                            // hard ellipsis-truncate: the widened backing (above) covers most real values, but
                            // nothing should be able to run an unbounded distance off the panel's own edge —
                            // a model/persona name is user data, not a tuned constant, so length isn't bounded.
                            let cur_disp: String = if cur.chars().count() > 11 {
                                cur.chars().take(10).chain(std::iter::once('.')).collect() // '…' has no glyph in glyph5x7; '.' does
                            } else { cur.clone() };
                            for mut cell in m3::label_cells(vcen, right, up, toward, &cur_disp, 0.010 * ps, t, egui::Color32::from_rgb(0x8a, 0xe0, 0xff), 0.62, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = ov_p + 0.15; graph.push(cell); }
                            if let Some(tr) = track_rect {
                                let resp = ui.interact(tr, egui::Id::new(("scene_phys_sel", id)), egui::Sense::click()).on_hover_text(format!("{label}: {cur} — click to cycle"));
                                if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                                if resp.clicked() {
                                    if let Some(crate::physui::ControlEvent::Selected(_, idx)) = self.phys.nudge_selector(id, 1) {
                                        match id {
                                            BACKEND_SEL => { self.backend = BackendKind::ALL[idx.min(1)]; }
                                            PERSONA_SEL => { if let Some((_, _, body)) = PERSONAS.get(idx) { self.context = (*body).to_string(); } }
                                            MODEL_SEL => { if let Some(m) = model.as_deref_mut() { if idx < m.items.len() { m.selected = idx; } } }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── IN-SCENE COMPOSER — the pearl, projected from a 3D-anchored slab ────────────────
                // Only needed when the 2D bottom bar is hidden (hide_bottom_bar): that's the toggle that
                // currently has no physical replacement, leaving no way to type at all in full-screen
                // spatial mode. Reuses pearl_input's proven cell-pool rendering via show_at(rect) — sized
                // from win_w/win_h/ppw like the dial panel and reading box above, instead of the old
                // draw_panel's fixed world offsets that put it below the visible frame on other aspect
                // ratios/window sizes. Only the text field + Enter-to-send are ported here; the fuller
                // attach/mic/export row stays 2D-only for now (toggle the bar back on to reach those).
                if self.hide_bottom_bar {
                    let comp_w = (0.62 * win_w / ppw).clamp(2.2, 6.5);
                    let comp_h = (0.20 * win_h / ppw).clamp(0.55, 1.3);
                    let comp_c = hud_place(&mut hud_placed, rect, &view_cam, right, up, cx, cy, scale,
                        anchor(0.5, 0.90), comp_w, comp_h, 6.0); // bottom-centre, adapts to window size
                    if let Some(rect) = project_slab_rect(&view_cam, comp_c, right, up, comp_w, comp_h, cx, cy, scale) {
                        let pearl_t = ui.input(|i| i.time) as f32;
                        let vpartial = self.voice.as_ref().map(|v| v.partial().to_string()).unwrap_or_default();
                        // scene accent (the panel idiom's text colour) so the composer pearl belongs to
                        // THIS world — Atlantis prismarine · Canopy leaf-glow · Mistfall rune-blue · Archive
                        // ink — rather than a fixed cyan. Same (tc) the mic pearl and reading plaque use.
                        let comp_accent = egui::Color32::from_rgb(tc[0], tc[1], tc[2]);
                        let band = self.pearl_input.show_at(ui, rect, &self.input, &vpartial, pearl_t, comp_accent);
                        ui.visuals_mut().text_cursor.stroke = egui::Stroke::NONE;
                        ui.visuals_mut().selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0x4d, 0xd6, 0xe6, 36);
                        let resp = ui.put(band, egui::TextEdit::multiline(&mut self.input)
                            // SAME salt at both call sites on purpose. This composer is
                            // drawn from two places (the wide and the narrow layout), and
                            // an auto-id would differ between them — so crossing the
                            // responsive breakpoint mid-sentence silently dropped focus
                            // and everything typed after it went nowhere. One stable id
                            // makes the two branches the same widget as far as focus is
                            // concerned.
                            .id_salt("logscope-chat-composer")
                            .id(egui::Id::new("logscope_chat_composer"))
                            .desired_width(f32::INFINITY)
                            .frame(egui::Frame::NONE)
                            .text_color(egui::Color32::TRANSPARENT)
                            .font(egui::FontId::new(14.0, egui::FontFamily::Monospace)));
                        let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                        if enter && self.input.ends_with('\n') { self.input.pop(); }
                        if resp.changed() {
                            self.mode = crate::vimmode::Mode::Insert;
                            if !resp.has_focus() { resp.request_focus(); }
                        }
                        if enter && !self.input.trim().is_empty() {
                            self.pearl_input.pulse();
                            send_now = true;
                        }

                        // ── PHYSICAL ACTION ROW — Attach / Mic / Send as in-scene slabs, mirroring the 2D
                        // composer's buttons so full-screen spatial mode (bottom bar hidden) can still attach,
                        // dictate, and send. A small three-button ledge just under the composer pearl.
                        let miced = self.voice.is_some();
                        let acts: [(&str, &str); 3] = [
                            ("ATT", "attach a text file"),
                            (if miced { "REC" } else { "MIC" }, if miced { "stop mic" } else { "dictate" }),
                            ("SEND", "send"),
                        ];
                        let btn_w = (0.08 * win_w / ppw).clamp(0.35, 0.9);
                        let gap = btn_w * 1.35;
                        let arow_c = hud_place(&mut hud_placed, rect, &view_cam, right, up, cx, cy, scale,
                            anchor(0.5, 0.985), gap * 2.0 + btn_w, btn_w * 0.8, 6.0); // just below the composer
                        for (i, (glyph, hint)) in acts.iter().enumerate() {
                            let dx = (i as f32 - 1.0) * gap; // centre the middle button
                            let bc = V3::new(arow_c.x + right.x * dx, arow_c.y + right.y * dx, arow_c.z + right.z * dx);
                            let mut btn = m3::panel(bc, right, up, btn_w, btn_w * 0.8, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                            btn.mat = pmat; btn.emissive = 0.14 * tglow * sheen; m3::set_alpha(&mut btn, 200); btn.overlay = 0.9;
                            graph.push(btn);
                            let gcen = V3::new(bc.x - toward.x * 0.30, bc.y - toward.y * 0.30, bc.z - toward.z * 0.30);
                            for mut cell in m3::label_cells(gcen, right, up, toward, glyph, 0.013 * (btn_w / 0.6), t, egui::Color32::from_rgb(tc[0], tc[1], tc[2]), 0.8, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = 1.05; graph.push(cell); }
                            if let Some(tr) = project_slab_rect(&view_cam, bc, right, up, btn_w * 1.15, btn_w * 0.95, cx, cy, scale) {
                                let resp = ui.interact(tr, egui::Id::new(("scene_action_btn", i)), egui::Sense::click()).on_hover_text(*hint);
                                if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                                if resp.clicked() {
                                    match i {
                                        0 => { // attach
                                            let start = self.save_dir.clone()
                                                .or_else(|| std::env::var("HOME").ok().map(std::path::PathBuf::from))
                                                .unwrap_or_else(|| std::path::PathBuf::from("/"));
                                            self.attach_picker.open_file(&start, "Attach a file", &[]);
                                        }
                                        1 => { // mic toggle
                                            if self.voice.is_some() { self.voice = None; }
                                            else {
                                                match crate::voice_session::VoiceSession::start() {
                                                    Ok(s) => { self.voice = Some(s); self.voice_err = None; ui.ctx().request_repaint(); }
                                                    Err(e) => { self.voice_err = Some(format!("voice: {e}")); }
                                                }
                                            }
                                        }
                                        _ => { // send
                                            if !self.input.trim().is_empty() { self.pearl_input.pulse(); send_now = true; }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── AGENT PLAN — the committed step list (2), rendered as a checklist that ticks off as
                // the run progresses, so the agent's intent is visible and you can steer/interrupt. Sits
                // just above the workspace-files feed on the right edge. Only when a plan exists.
                //
                // PLAN_BOTTOM_FRAC carries PLAN's actual last-row screen position out of this block so
                // WORKSPACE (below) can anchor directly below it — see that block for why this replaced
                // an attempt to route both through `hud_place`'s screen-space collision system: two rounds
                // of tuning the footprint size there first under-corrected (still overlapping) then over-
                // corrected (WORKSPACE pushed UP into PLAN from the other side) without converging, so this
                // computes the real answer directly instead of iterating on a collision-system guess.
                let mut plan_bottom_frac: Option<f32> = None;
                if !self.backdrop && !self.agent_plan.is_empty() {
                    let pc0 = anchor(0.86, 0.13);
                    let pw = (0.22 * win_w / ppw).clamp(0.8, 2.4);
                    let pscale = pw / 1.3;
                    let title_c = V3::new(pc0.x - toward.x * 0.30, pc0.y - toward.y * 0.30, pc0.z - toward.z * 0.30);
                    for mut cell in m3::label_cells(title_c, right, up, toward, "PLAN", 0.010 * pscale, t, egui::Color32::from_rgb(232, 238, 252), 0.7, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = 0.9; graph.push(cell); }
                    let n_rows = self.agent_plan.len().min(8);
                    for (row, (text, done)) in self.agent_plan.iter().take(8).enumerate() {
                        let off = -(row as f32 + 1.0) * 0.15 * pscale;
                        let rc = V3::new(pc0.x + up.x * off - toward.x * 0.30, pc0.y + up.y * off - toward.y * 0.30, pc0.z + up.z * off - toward.z * 0.30);
                        let mark = if *done { "[x] " } else { "[ ] " };
                        let col = if *done { egui::Color32::from_rgb(tc[0], tc[1], tc[2]) } else { egui::Color32::from_rgb(150, 160, 178) };
                        let line: String = format!("{mark}{}", text.chars().take(18).collect::<String>());
                        for mut cell in m3::label_cells(rc, right, up, toward, &line, 0.008 * pscale, t, col, if *done { 0.75 } else { 0.5 }, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = 0.9; graph.push(cell); }
                    }
                    // Project the ACTUAL last row's world position back to screen space, rather than
                    // estimating a footprint for a collision system: this is the exact, direct answer.
                    let last_off = -(n_rows as f32) * 0.15 * pscale;
                    let last_pos = V3::new(pc0.x + up.x * last_off, pc0.y + up.y * last_off, pc0.z + up.z * last_off);
                    if let Some((_, sy, _)) = view_cam.project(last_pos, cx, cy, scale) {
                        plan_bottom_frac = Some(((sy - rect.top()) / win_h).max(0.13));
                    }
                }

                // ── WORKSPACE FILES — live feed of files the agent wrote / you dropped, so file creation
                // is VISIBLE in the scene (the ask: "see file creation"). Right-edge plaque, newest last,
                // colour-coded (created = accent, modified = dim). Only when a session has touched files.
                if !self.backdrop && !self.agent_files.is_empty() {
                    let fw = (0.22 * win_w / ppw).clamp(0.8, 2.4);
                    let fs = fw / 1.3;
                    // Anchored below PLAN's real (projected) bottom edge + a fixed margin, when PLAN is
                    // showing; otherwise the original fixed fraction. A direct answer, not a guessed
                    // footprint — see the comment on `plan_bottom_frac` above for why this replaced the
                    // `hud_place` attempt.
                    let fc = anchor(0.86, plan_bottom_frac.map(|f| f + 0.06).unwrap_or(0.42).min(0.85));
                    let back = |o: f32| V3::new(fc.x - toward.x * o, fc.y - toward.y * o, fc.z - toward.z * o);
                    let mut plaque = m3::panel(back(-0.06 * fs), right, up, 1.3 * fs, 1.7 * fs, egui::Color32::from_rgb(pc[0], pc[1], pc[2]));
                    plaque.mat = pmat; plaque.emissive = 0.07 * tglow * sheen; m3::set_alpha(&mut plaque, 150); plaque.overlay = 0.9;
                    graph.push(plaque);
                    let title_c = V3::new(fc.x - toward.x * 0.30 + up.x * 0.72 * fs, fc.y - toward.y * 0.30 + up.y * 0.72 * fs, fc.z - toward.z * 0.30 + up.z * 0.72 * fs);
                    for mut cell in m3::label_cells(title_c, right, up, toward, "WORKSPACE", 0.010 * fs, t, egui::Color32::from_rgb(232, 238, 252), 0.7, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = 1.05; graph.push(cell); }
                    let n = self.agent_files.len();
                    let show = self.agent_files.iter().rev().take(8).collect::<Vec<_>>();
                    for (row, ev) in show.iter().enumerate() {
                        let off = (0.52 - row as f32 * 0.15) * fs;
                        let rc = V3::new(fc.x + up.x * off - toward.x * 0.30, fc.y + up.y * off - toward.y * 0.30, fc.z + up.z * off - toward.z * 0.30);
                        let created = matches!(ev.change, crate::workspace_exec::FileChange::Created);
                        let col = if created { egui::Color32::from_rgb(tc[0], tc[1], tc[2]) } else { egui::Color32::from_rgb(150, 160, 178) };
                        let mark = if created { "+ " } else { "~ " };
                        let name: String = ev.path.rsplit('/').next().unwrap_or(&ev.path).chars().take(16).collect();
                        for mut cell in m3::label_cells(rc, right, up, toward, &format!("{mark}{name}"), 0.008 * fs, t, col, if created { 0.75 } else { 0.5 }, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = 1.05; graph.push(cell); }
                    }
                    if n > 8 {
                        let mc = V3::new(fc.x + up.x * (0.52 - 8.0 * 0.15) * fs - toward.x * 0.30, fc.y + up.y * (0.52 - 8.0 * 0.15) * fs - toward.y * 0.30, fc.z + up.z * (0.52 - 8.0 * 0.15) * fs - toward.z * 0.30);
                        for mut cell in m3::label_cells(mc, right, up, toward, &format!("+{} more", n - 8), 0.008 * fs, t, egui::Color32::from_rgb(120, 130, 148), 0.5, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = 1.05; graph.push(cell); }
                    }
                }

                // ── CANDIDATES — the fan's folded branches as a clickable prune panel. Each card shows the
                // branch's review verdict (✓ approved / ~ verified / · plain), file count, and a short review
                // note; the reviewer-suggested branch is ringed. Click a card to LAND that branch (:pick).
                // This is the fold/prune decision made visual — the keeper is one click, not a typed command.
                if !self.backdrop && !self.agent_candidates.is_empty() {
                    let cw = (0.30 * win_w / ppw).clamp(1.4, 3.6);
                    let cs = cw / 2.0;
                    // Cards are centred ON cc but the anchor (0.17, near the left edge) never accounted for
                    // half the card width extending further left — a wide card ran the panel's own title
                    // ("PICK A BRANCH") off the left edge of the screen entirely (screenshot comparison).
                    // hud_place's clamp-into-viewport (same mechanism protecting the dock/PARAMS panel) fixes
                    // this the same way: give it the true footprint and let it push the centre clear of the edge.
                    let n_rows = self.agent_candidates.len().min(6);
                    let cc = hud_place(&mut hud_placed, rect, &view_cam, right, up, cx, cy, scale,
                        anchor(0.17, 0.22), cw * 1.1, (n_rows as f32 + 1.0) * 0.30 * cs, 6.0);
                    let best = self.agent_candidates.iter().max_by_key(|b| (b.approved == Some(true), b.verified, b.files)).map(|b| b.index);
                    let title_c = V3::new(cc.x - toward.x * 0.30 + up.x * 0.5 * cs, cc.y - toward.y * 0.30 + up.y * 0.5 * cs, cc.z - toward.z * 0.30 + up.z * 0.5 * cs);
                    for mut cell in m3::label_cells(title_c, right, up, toward, "PICK A BRANCH", 0.011 * cs, t, egui::Color32::from_rgb(232, 238, 252), 0.75, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = 1.05; graph.push(cell); }
                    let mut clicked_pick: Option<usize> = None;
                    let rowh = 0.30 * cs;
                    for (row, cand) in self.agent_candidates.iter().take(6).enumerate() {
                        let off = -(row as f32) * rowh;
                        let rc = V3::new(cc.x + up.x * off, cc.y + up.y * off, cc.z + up.z * off);
                        let is_best = best == Some(cand.index);
                        // card slab (accent-tinted for the suggested keeper)
                        let (cr, cg, cb) = if is_best { (tc[0], tc[1], tc[2]) } else { (pc[0], pc[1], pc[2]) };
                        let mut card = m3::panel(rc, right, up, cw, rowh * 0.82, egui::Color32::from_rgb(cr, cg, cb));
                        card.mat = pmat; card.emissive = if is_best { 0.28 } else { 0.12 } * tglow * sheen; m3::set_alpha(&mut card, if is_best { 210 } else { 170 }); card.overlay = 0.9;
                        graph.push(card);
                        // status glyph + label
                        let (mark, mcol) = match cand.approved {
                            Some(true) => ("[OK]", egui::Color32::from_rgb(140, 230, 150)),
                            Some(false) => ("[--]", egui::Color32::from_rgb(230, 150, 140)),
                            None if cand.verified => ("[~]", egui::Color32::from_rgb(tc[0], tc[1], tc[2])),
                            None => ("[ ]", egui::Color32::from_rgb(160, 170, 188)),
                        };
                        let head = format!("{mark} {} · {}f", cand.label.chars().take(14).collect::<String>(), cand.files);
                        let hc = V3::new(rc.x - toward.x * 0.30 + up.x * 0.06 * cs, rc.y - toward.y * 0.30 + up.y * 0.06 * cs, rc.z - toward.z * 0.30 + up.z * 0.06 * cs);
                        for mut cell in m3::label_cells(hc, right, up, toward, &head, 0.0085 * cs, t, mcol, 0.8, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = 1.05; graph.push(cell); }
                        // review note (one truncated line under the head)
                        if !cand.review_notes.is_empty() {
                            let note: String = cand.review_notes.lines().next().unwrap_or("").chars().take(28).collect();
                            let nc = V3::new(rc.x - toward.x * 0.30 - up.x * 0.06 * cs, rc.y - toward.y * 0.30 - up.y * 0.06 * cs, rc.z - toward.z * 0.30 - up.z * 0.06 * cs);
                            for mut cell in m3::label_cells(nc, right, up, toward, &note, 0.0072 * cs, t, egui::Color32::from_rgb(150, 160, 178), 0.55, 0.0, tanim, m3::SceneLight::Uniform, 0) { cell.overlay = 1.05; graph.push(cell); }
                        }
                        // click → land this branch
                        if let Some(tr) = project_slab_rect(&view_cam, rc, right, up, cw * 1.05, rowh * 0.9, cx, cy, scale) {
                            let hint = format!("branch {} · {} file(s){}{} — click to land it",
                                cand.index + 1, cand.files,
                                if cand.verified { " · verified" } else { "" },
                                match cand.approved { Some(true) => " · approved", Some(false) => " · needs revision", None => "" });
                            let resp = ui.interact(tr, egui::Id::new(("fan_candidate", cand.index)), egui::Sense::click()).on_hover_text(hint);
                            if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                            if resp.clicked() { clicked_pick = Some(cand.index + 1); }
                        }
                    }
                    if let Some(n) = clicked_pick { self.pick_branch(n); }
                }

                // ── MESSAGE READING BOX — the focused turn as a physical env-styled plaque ──────────
                // Clicking a node focuses it; its turn text then reads as a real in-world plaque (bezel + shadow
                // + slab + wrapped label_cells text) carrying the scene's palette/material/emissive — an object,
                // not a 2D overlay. (LOGSCOPE_FOCUS=<n> forces a focus for headless checks.)
                if let Some(fid) = focused_id.filter(|i| *i < self.msgs.len()) {
                    let (who, raw) = { let m = &self.msgs[fid]; (if m.role == Role::User { "you" } else { "assistant" }, m.text().to_string()) };
                    let head = format!("{who} · turn {}/{}", fid + 1, n);
                    // Reset the glide (don't inherit scroll position/animation) when focus moves to a
                    // DIFFERENT message this frame.
                    if self.plaque_last_focus != Some(fid) { self.plaque_scroll.reset(); self.plaque_last_focus = Some(fid); }
                    let all_lines = wrap_label(&raw, 24, usize::MAX);
                    let max_line = all_lines.len().saturating_sub(PLAQUE_WINDOW) as f32;
                    let dt = ui.input(|i| i.stable_dt).min(0.1); // clamp a stalled first frame's huge dt
                    let pos = self.plaque_scroll.tick(dt).clamp(0.0, max_line);
                    if !self.plaque_scroll.at_rest() { ui.ctx().request_repaint(); } // keep animating the glide
                    let start = pos.round() as usize;
                    let lines: Vec<String> = all_lines.iter().skip(start).take(PLAQUE_WINDOW).cloned().collect();
                    // scroll affordance: show "▲ n above" / "▼ n below" so scrolled state is legible in-scene.
                    let head = if all_lines.len() > PLAQUE_WINDOW {
                        let above = start; let below = all_lines.len().saturating_sub(start + lines.len());
                        let marks = match (above > 0, below > 0) {
                            (true, true) => " ▲▼", (true, false) => " ▲", (false, true) => " ▼", (false, false) => "",
                        };
                        format!("{head}{marks}  ⟨j/k to scroll⟩")
                    } else { head };
                    let rc = anchor(0.5, 0.17); // top-centre, adapts to window size
                    // SETTLE — the plaque doesn't just appear at rest; it arrives WITH the pearl (same
                    // `focus_pull` driving the pull-toward-camera animation on the node itself), popping
                    // in from a slightly smaller scale with its drop-shadow thrown further out, both
                    // converging to their resting values as focus_pull reaches 1. A physical object that's
                    // still settling casts a longer, softer-anchored shadow than one at rest — this is the
                    // same depth cue, applied to the reading surface instead of the pearl. `settle` is 0
                    // once fully arrived, so nothing here changes the at-rest appearance already verified.
                    let settle = (1.0 - self.focus_pull).clamp(0.0, 1.0);
                    let scale_pop = 1.0 - 0.10 * settle; // 0.90 on arrival → 1.0 at rest
                    let pw = ((0.44 * win_w / ppw).clamp(1.6, 5.2)) * scale_pop;
                    let ph = ((0.44 * win_w / ppw).clamp(1.6, 5.2) * 0.42).min(0.30 * win_h / ppw) * scale_pop; // cap by height so it fits short windows
                    let bscale = (pw / (24.0 * 6.0) * 0.90).clamp(0.006, 0.03); // body fits ~24 cols
                    let hscale = bscale * 0.82;
                    // drop shadow (offset down-right, set back) for real depth — separation grows while unsettled
                    let shadow_mul = 1.0 + 0.8 * settle;
                    let scen = V3::new(rc.x + fwd.x * 0.16 * shadow_mul + right.x * 0.10 * shadow_mul - up.x * 0.10 * shadow_mul, rc.y + fwd.y * 0.16 * shadow_mul + right.y * 0.10 * shadow_mul - up.y * 0.10 * shadow_mul, rc.z + fwd.z * 0.16 * shadow_mul + right.z * 0.10 * shadow_mul - up.z * 0.10 * shadow_mul);
                    let mut shadow = m3::panel(scen, right, up, pw + 0.5, ph + 0.4, egui::Color32::from_rgb(0, 0, 0));
                    shadow.mat = crate::texgen::mat::STONE; shadow.emissive = 0.0; m3::set_alpha(&mut shadow, (96.0 * (1.0 - 0.35 * settle)) as u8); graph.push(shadow);
                    // bezel + slab in the scene palette/material
                    let bez = egui::Color32::from_rgb((pc[0] as f32 * 0.5) as u8, (pc[1] as f32 * 0.5) as u8, (pc[2] as f32 * 0.5) as u8);
                    let fcen = V3::new(rc.x + fwd.x * 0.05, rc.y + fwd.y * 0.05, rc.z + fwd.z * 0.05);
                    // OVERLAY BIAS: the plaque is physical UI floating in front of the world — pull it
                    // toward the camera for depth so canopy leaves / props between it and the graph can't
                    // occlude it, while its own bezel→slab→text still layer correctly among themselves.
                    let ov = 0.9_f32;
                    let mut frame = m3::panel(fcen, right, up, pw + 0.28, ph + 0.24, bez); frame.mat = pmat; frame.emissive = pemis * 0.4; m3::set_alpha(&mut frame, (palpha as u16 + 50).min(255) as u8); frame.overlay = ov; graph.push(frame);
                    // CORNER ORNAMENT — every reference concept image (Atlantis, Observatory, Mansion
                    // Garden, Alley) frames its message plaques with a metal/gilt corner bracket, not a
                    // flat monochrome bezel alone. Four small accent squares in the scene's own accent
                    // colour, pulled a hair closer than the frame so they read as an inlay rather than
                    // fighting it in the depth buffer. Cheap (4 extra quads) and purely decorative — no
                    // interaction/hit-testing changes.
                    {
                        let acc = theme::accent();
                        let (hw, hh) = (pw * 0.5 + 0.14, ph * 0.5 + 0.12); // just inside the frame's own edge
                        let cs = (pw.min(ph) * 0.09).clamp(0.05, 0.18); // corner accent size, scales with the plaque
                        let ccen = V3::new(fcen.x + fwd.x * 0.01, fcen.y + fwd.y * 0.01, fcen.z + fwd.z * 0.01);
                        // ARRIVAL GLOW — the corners run brighter than their resting emissive while the plaque
                        // is still settling (SAME `settle` signal as the scale-pop/shadow above), fading to
                        // the normal resting glow once arrived — settle² so it falls away fast near the end
                        // instead of a linear fade the eye reads as just "dimming slowly" rather than settling.
                        let arrival_flare = settle * settle * 1.8;
                        for (sx, sy) in [(-1.0_f32, 1.0_f32), (1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
                            let p = V3::new(ccen.x + right.x * hw * sx + up.x * hh * sy, ccen.y + right.y * hw * sx + up.y * hh * sy, ccen.z + right.z * hw * sx + up.z * hh * sy);
                            let mut corner = m3::panel(p, right, up, cs, cs, acc);
                            corner.mat = pmat; corner.emissive = (pemis * sheen).max(0.5) + arrival_flare; corner.overlay = ov + 0.01;
                            graph.push(corner);
                        }
                    }
                    let mut slab = m3::panel(rc, right, up, pw, ph, egui::Color32::from_rgb(pc[0], pc[1], pc[2])); slab.mat = pmat; slab.emissive = pemis * sheen; m3::set_alpha(&mut slab, palpha); slab.overlay = ov; graph.push(slab); slab_glint(&mut graph, rc, right, up, toward, pw * 0.5, ph * 0.5, sun, tglow);
                    // left-edge origin: label_cells centres on its origin, so offset each line by +half its width
                    let left_edge = pw * 0.5 - pw * 0.06;
                    // the plaque's text is lit by the CURRENT SCENE's physical light source (dappled
                    // canopy leaves, a rotating pearl's facets, …) rather than a flat dot-matrix glow —
                    // see mesh3d::SceneLight's doc comment for the full per-scene ledger. Seeded by the
                    // focused message id so different messages' motes/glints don't move in lockstep.
                    let scene_light = crate::mesh3d::scene_light_for(theme::active().name);
                    let plaque_seed = fid as u32;
                    let put_line = |g: &mut Vec<crate::mesh3d::Mesh3>, text: &str, y: f32, scale: f32, col: egui::Color32, lit: f32| {
                        let w = text.chars().count() as f32 * 6.0 * scale;
                        let o = V3::new(rc.x - toward.x * 0.30 - right.x * left_edge + right.x * (w * 0.5) + up.x * y,
                                        rc.y - toward.y * 0.30 - right.y * left_edge + right.y * (w * 0.5) + up.y * y,
                                        rc.z - toward.z * 0.30 - right.z * left_edge + right.z * (w * 0.5) + up.z * y);
                        for mut cell in m3::label_cells(o, right, up, toward, text, scale, t, col, lit, 0.0, tanim, scene_light, plaque_seed) { cell.overlay = 1.05; g.push(cell); }
                    };
                    put_line(&mut graph, &head, ph * 0.5 - ph * 0.13, hscale, egui::Color32::from_rgb(0xff, 0xc2, 0x6a), 0.85);
                    for (li, line) in lines.iter().enumerate() {
                        put_line(&mut graph, line, ph * 0.5 - ph * 0.38 - li as f32 * ph * 0.17, bscale, egui::Color32::from_rgb(tc[0], tc[1], tc[2]), 0.85 * tglow.max(0.5));
                    }
                    if let Some(pr) = project_slab_rect(&view_cam, rc, right, up, pw, ph, cx, cy, scale) {
                        let resp = ui.interact(pr, egui::Id::new("scene_reader"), egui::Sense::click())
                            .on_hover_text(format!("{head}: {}", raw.chars().take(160).collect::<String>()));
                        // Claim this press for the plaque's OWN world position (rc), not wherever the
                        // floor/graph raycast lands behind it — checked against the same rect used for
                        // hit-testing, at raw-press time (not `.clicked()`, which fires on release and
                        // would miss the frame the generic handler below reads).
                        if let Some(pp) = ui.input(|i| if i.pointer.primary_pressed() { i.pointer.interact_pos() } else { None }) {
                            if pr.contains(pp) { touch_override = Some([rc.x, rc.y, rc.z]); }
                        }
                        // RIGHT-CLICK CONTEXT MENU — the plaque is a real object, so a secondary click opens
                        // actions on the focused turn (copy, jump the composer to it, clear focus). This is the
                        // in-scene equivalent of a desktop context menu, driven off the same projected rect.
                        resp.context_menu(|ui| {
                            if ui.button("Copy turn text").clicked() {
                                ui.ctx().copy_text(raw.clone());
                                ui.close_menu();
                            }
                            if ui.button(format!("Reference :{} in composer", fid + 1)).clicked() {
                                if !self.input.ends_with(' ') && !self.input.is_empty() { self.input.push(' '); }
                                self.input.push_str(&format!(":{} ", fid + 1));
                                ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("logscope_chat_composer")));
                                ui.close_menu();
                            }
                            if ui.button("Clear focus").clicked() {
                                self.walk_focus = None;
                                ui.close_menu();
                            }
                        });
                    }
                }
            }

            // CPU path (default): software-shaded triangles into egui's painter.
            #[cfg(not(feature = "gpu-scene"))]
            {
                crate::scene3d::render_env(&p, &view_cam, cx, cy, scale, name, &light, t);
                if !world_only { p.add(m3::render(&graph, &view_cam, cx, cy, scale, &light)); }
            }
            // GPU path (`--features gpu-scene`): one depth-sorted GLSL pass (env behind graph) via egui_glow,
            // pixel-aligned with the dots/runes because the camera matrix reproduces OrbitCam::project.
            #[cfg(feature = "gpu-scene")]
            {
                // DEPTH RANGE FITTED TO THE SCENE. This was a hardcoded
                // (0.3, 400.0) whatever was being drawn — a 1333:1 near/far
                // ratio, and a perspective depth buffer spends its precision
                // near the eye, so the far half of a plaza had almost none
                // left. Columns a distance apart landed in the same depth
                // bucket and flickered: reported as "cylinder and circle
                // z-fighting across the spaces".
                // The far plane has to reach the WORLD's edge. This was `ext/2 + 8` — the graph's extent —
                // so with any camera nearer than the outer wall the wall, the sky and the boundary ring were
                // clipped away: a cave whose floor and walls were black past forty units, a pond that was not
                // there. Found by rendering the Cat Cave; the same cut applies to every scene whose props sit
                // beyond the graph (all of them). The scene knows its own radius; ask it.
                let scene_r = crate::scene3d::scene_radius(name).max(20.0);
                let (znear, zfar) = crate::gpuscene::Camera::depth_range(scene_r, view_cam.dist);
                let cam = crate::gpuscene::Camera::from_orbit(&view_cam, rect.width(), rect.height(), scale, znear, zfar);
                let mut verts = Vec::new();
                // dynamic-LOD screen-size threshold (NDC) for SCENERY: drop meshes too small to read; more
                // aggressive under battery-saver POWER; tunable via LOGSCOPE_LOD, off via LOGSCOPE_NO_LOD=1.
                let lod_ndc = if std::env::var("LOGSCOPE_NO_LOD").map(|v| v == "1").unwrap_or(false) { 0.0 }
                    else { std::env::var("LOGSCOPE_LOD").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0035) / crate::theme::power_mode().max(0.25) };
                crate::scene3d::with_env(name, t, |env| verts.extend(crate::gpuscene::pack_sorted(env, &cam, lod_ndc)));
                if !world_only { verts.extend(crate::gpuscene::pack_sorted(&graph, &cam, 0.0)); } // graph: no LOD (keep every node), drawn in front
                let sun = m3::sun_dir_3d();
                let c3 = |c: egui::Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0];
                let sun_uv = cam.dir_uv([sun.x, sun.y, sun.z]).unwrap_or([0.5, 1.25]); // above-screen if behind → faint
                let (gexp, gfogm, gbloom, gthr, gambm, gwarm) = theme::grade(theme::LOCKED_GRADE);
                let (_texn, dtexscale, dbump, _tess) = theme::detail(theme::variant()); // detail sweep → finer tiling + relief
                let fog = match name { // base fog per theme, then scaled by the locked grade
                    "Mistfall" => 0.044,  // still a misty veil, but you can see across the room
                    "Treehouse" => 0.022, // dusk haze blots distant trees (LOD by fog, not walls)
                    "Folding@home" => 0.014,
                    "Atlantis" => 0.009, // AIR interior reads clear; the water surfaces carry their own depth murk
                    "Cat Cave" => 0.006, // a cave's air is clear: the dark is where the lanterns do not reach, not haze
                    "Forest Village" => 0.016, // light rain: the far balconies soften
                    _ => 0.011,           // Archive/Slate/Old Library/Reading Room: light haze only (was 0.017 → grey-out)
                } * gfogm * std::env::var("LOGSCOPE_FOGM").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0);
                // warm/cool shift the sun for the grade
                let warm_target = if gwarm >= 0.0 { egui::Color32::from_rgb(255, 224, 170) } else { egui::Color32::from_rgb(180, 205, 255) };
                let sun_graded = theme::lerp_col(light.color, warm_target, gwarm.abs());
                // TIME-OF-DAY sync: the local clock drives sun strength, warmth and fill on top of the grade —
                // warm low sun at dawn/dusk, bright neutral midday, dim cool at night.
                let (tod_strength, tod_warm, tod_amb) = m3::sun_clock();
                let tod_target = if tod_warm >= 0.0 { egui::Color32::from_rgb(255, 210, 150) } else { egui::Color32::from_rgb(150, 180, 255) };
                let sun_tod = theme::lerp_col(sun_graded, tod_target, tod_warm.abs() * 0.8);
                let caustic = match name { "Atlantis" => 0.32, "Mistfall" => 0.2, _ => 0.0 };
                let aquatic = name == "Atlantis";
                let sun_uv = if aquatic { [0.5, 0.10] } else { sun_uv }; // shafts pour down from the oculus overhead
                // gr_amt was 0.9 for Atlantis — the highest god-ray weight in any theme — and on top of
                // this scene's own caustic + underwater-tint passes it blew the whole dome out to a flat
                // cyan wash (screenshot A/B, fixed camera: LOGSCOPE_GODRAY=0.4 vs default, llvmpipe).
                // Trimmed to match the bloom trim two scenes below already gets. gr_w/gr_dec (shaft
                // width/falloff shape) are untouched — this is an amplitude fix, not a shape fix.
                let (gr_amt, gr_w, gr_dec) = if aquatic { (0.36_f32, 0.52_f32, 0.95_f32) } else { (0.6, 0.45, 0.92) };
                // COMPOSER PEARL — env-driven for now (headless preview of the refractive pearl in-scene); the
                // interactive "cast your next pearl into the scene" flow will drive these live.
                let pearl = std::env::var("LOGSCOPE_PEARL").ok().and_then(|v| v.trim().parse::<f32>().ok())
                    .unwrap_or(if self.pearl_on { 1.0 } else { 0.0 });
                let pearl_variant = std::env::var("LOGSCOPE_PEARL_VARIANT").ok().and_then(|v| v.trim().parse::<f32>().ok())
                    .unwrap_or(self.pearl_variant as f32);
                let pearl_pos = std::env::var("LOGSCOPE_PEARL_POS").ok()
                    .and_then(|v| { let q: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect(); if q.len() == 2 { Some([q[0], q[1]]) } else { None } })
                    .unwrap_or([0.5, 0.40]);
                let pearl_r = std::env::var("LOGSCOPE_PEARL_R").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.26);
                // UNDERWATER: warps the view when the eye is in OPEN WATER. Procedural gen means "outside the
                // main Bound" is no longer the test — hallways + wings (at the Gate bearings) extend past the
                // Bound but are AIR. So: ramp on past the perimeter ONLY where the eye is NOT within a Gate
                // corridor (the openings the fly travels out through). Inside the room or any hallway/wing → dry.
                let underwater = if aquatic {
                    let e = cam.eye();
                    let cols = crate::scene3d::env_colliders();
                    let rp = cols.iter().find_map(|c| match c { crate::scene3d::Collider::Bound { r } => Some(*r), _ => None }).unwrap_or(f32::MAX);
                    let d = (e[0] * e[0] + e[2] * e[2]).sqrt();
                    if d <= rp - 1.0 {
                        0.0 // inside the main rotunda — always air
                    } else {
                        let theta = e[2].atan2(e[0]);
                        let ang_d = |a: f32, b: f32| { let mut x = (a - b).abs() % std::f32::consts::TAU; if x > std::f32::consts::PI { x = std::f32::consts::TAU - x; } x };
                        // a Gate corridor (hallway/wing) is air even past the Bound — a generous wedge covers the
                        // ellipse z-compression that skews the apparent bearing.
                        let in_corridor = cols.iter().any(|c| match c {
                            crate::scene3d::Collider::Gate { ang, half } => ang_d(theta, *ang) < half + 0.35,
                            _ => false,
                        });
                        if in_corridor { 0.0 } else { ((d - (rp - 1.0)) / 3.5).clamp(0.0, 1.0) }
                    }
                } else { 0.0 };
                // TOUCH RESPONSE — wire the dead u_touch. A primary press inside the scene casts a
                // ray (gpuscene::ray_uv, the tested inverse of dir_uv) and anchors the glow where it
                // meets the floor plane, or at orbit distance when pointing at sky — a mid-air glow
                // still answers the finger. RAW pointer input rather than the orbit Response, on
                // purpose: in-scene slabs consume clicks through their own interacts, and pressing a
                // slab IS touching it — the physical UI should glow under every touch, not only the
                // ones no control wanted. egui y grows DOWN, ndc-uv y grows UP: the flip lives here,
                // at the screen boundary, so gpuscene stays in one convention.
                //
                // `touch_override`, if set above, means the press landed on the reading plaque — an
                // OVERLAY-biased object pulled toward the camera, which sits at a different depth than
                // whatever the raycast would hit behind it. Honor that instead of the raycast so the
                // plaque glows at its own surface, not the floor/graph seen through it.
                if let Some(tv) = touch_override {
                    self.scene_touch = Some((tv, t));
                } else if let Some(pp) = ui.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None }) {
                    if rect.contains(pp) {
                        let uv = [(pp.x - rect.left()) / rect.width().max(1.0),
                                  1.0 - (pp.y - rect.top()) / rect.height().max(1.0)];
                        let (o, d) = cam.ray_uv(uv);
                        let hit = if d[1] < -1e-4 {
                            let th = (-o[1] / d[1]).clamp(0.5, 80.0);
                            [o[0] + d[0] * th, o[1] + d[1] * th, o[2] + d[2] * th]
                        } else {
                            [o[0] + d[0] * view_cam.dist, o[1] + d[1] * view_cam.dist, o[2] + d[2] * view_cam.dist]
                        };
                        self.scene_touch = Some((hit, t));
                    }
                }
                // age on the same scene clock the shader animates with; expire past the shader's fade
                let touch = self.scene_touch.and_then(|(p2, t0)| {
                    let age = t - t0;
                    if age > 3.0 { None } else { Some([p2[0], p2[1], p2[2], age]) }
                });
                let u = crate::glow_scene::Uniforms {
                    touch,
                    viewproj: cam.view_proj(), eye: cam.eye(),
                    sun_dir: [sun.x, sun.y, sun.z], sun_col: c3(sun_tod),
                    sky: c3(light.sky), ground: c3(light.ground), ambient: light.ambient * gambm * tod_amb,
                    fog_col: c3(theme::active().bg), fog, sun_uv, time: t,
                    exposure: gexp * lexp * (if aquatic { 1.12 } else { 1.0 }) * (0.55 + 0.48 * tod_strength)
                        * match name { "Recursive Citadel" => 0.84, "Reading Room" => 0.9, "Folding@home" => 0.86, _ => 1.0 } // trim the over-bright worlds
                        * std::env::var("LOGSCOPE_EXP").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0),
                    saturation: std::env::var("LOGSCOPE_SAT").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.18),
                    shoulder: std::env::var("LOGSCOPE_SHOULDER").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0),
                    // per-scene BLOOM trim — Old Library + Cloudscape were over-blooming (detail washed out).
                    // Atlantis was missing from this list despite the same profile that earned the others a
                    // trim (pale coral/marble surfaces filling most of frame) PLUS its own caustic + underwater-
                    // tint passes stacking additional brightness on top — screenshot comparison (close-range
                    // llvmpipe render) showed it blowing out to a near-solid cyan wash; disabling the tonemap
                    // shoulder made it WORSE (near-white), which rules out the tonemap and points upstream, here.
                    // LOGSCOPE_BLOOM / LOGSCOPE_GODRAY are diagnostic MULTIPLIERS (default 1.0 = no change),
                    // in the same idiom as LOGSCOPE_SAT / LOGSCOPE_SHOULDER above. `LOGSCOPE_NO_POST` turns the
                    // whole chain off at once, which proves a tint is post rather than scene but cannot say
                    // WHICH pass; these two split it without a rebuild.
                    threshold: gthr * 1.0,
                    bloom_amt: gbloom * 0.85 * match name { "Old Library" => 0.5, "Cloudscape" => 0.42, "Reading Room" => 0.4, "Recursive Citadel" => 0.5, "Folding@home" => 0.4, "Mansion Garden" => 0.26, "Atlantis" => 0.4, _ => 1.0 }
                        * std::env::var("LOGSCOPE_BLOOM").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0),
                    texscale: dtexscale, bump: dbump,
                    caustic,
                    godray_amt: gr_amt * std::env::var("LOGSCOPE_GODRAY").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0),
                    godray_weight: gr_w, godray_decay: gr_dec,
                    pearl, pearl_variant, pearl_pos, pearl_r, aspect: rect.width() / rect.height().max(1.0),
                    nacre: std::env::var("LOGSCOPE_NACRE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.4),
                    underwater,
                    // frost strength: the glass-UI backdrop's flat frost, OR — same knob, physical-UI use —
                    // the background blurring away as a focused pearl is pulled to reading distance. The two
                    // never overlap in practice (backdrop mode has no focused turn) so max() picks whichever
                    // is asking; the pearl's own read is kept via `focus` below rather than the frost bands.
                    defocus: (if self.backdrop { 0.85_f32 } else { 0.0 }).max(0.6 * self.focus_pull),
                    frost_a: self.frost_a, frost_b: self.frost_b,    // ...but only within the chrome bands
                    cam_warp: [view_cam.yaw, view_cam.pitch],         // 3D-aware underwater field follows the look angle
                    // backdrop is opaque so no undefined framebuffer shows through; fill empty sky with the theme bg
                    opaque: if self.backdrop { 1.0 } else { 0.0 },
                    bg: { let b = theme::active().bg; [b.r() as f32 / 255.0 * 0.7, b.g() as f32 / 255.0 * 0.7, b.b() as f32 / 255.0 * 0.7] },
                    // DEPTH-OF-FIELD: focus on the content (orbit distance), defocus the far background.
                    // When a turn's pearl is pulled toward the camera, slide the focal plane toward IT
                    // instead — the pearl stays crisp at its new, closer distance while the rest of the
                    // room falls away, which is what sells "pulled out of the graph" rather than "zoomed".
                    dof: std::env::var("LOGSCOPE_DOF").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.7),
                    focus: {
                        let base = view_cam.dist;
                        match focused_id {
                            Some(fid) if self.focus_pull > 0.001 => {
                                let p = pos[fid];
                                let d = ((eye_now.x - p.x).powi(2) + (eye_now.y - p.y).powi(2) + (eye_now.z - p.z).powi(2)).sqrt();
                                let pulled_d = (d - (view_cam.dist * 0.35).min(2.6) * self.focus_pull).max(0.6);
                                base + (pulled_d - base) * self.focus_pull
                            }
                            _ => base,
                        }
                    },
                    // SHADOWS: ortho shadow map along the sun, CENTERED ON THE CAMERA TARGET (follows the view)
                    // with a doubled radius for wider coverage. Toggle off with LOGSCOPE_SHADOWS=0.
                    light_viewproj: crate::gpuscene::light_view_proj(
                        [sun.x, sun.y, sun.z],
                        [view_cam.target.x, view_cam.target.y, view_cam.target.z], 128.0),
                    shadow_on: if std::env::var("LOGSCOPE_SHADOWS").map(|v| v == "0").unwrap_or(false) { 0.0 } else { 1.0 },
                    // COLORED FILL LIGHT: a soft directional fill from the off-sun side, tinted by the scene's
                    // ambient sky colour — so the shadowed side picks up the scene's hue (aqua underwater, green
                    // under canopy, moonlit blue, …) instead of going flat-grey. Complements the per-theme sun.
                    fill_dir: { let (x, y, z) = (-sun.x * 0.8 + 0.25, 0.5, -sun.z * 0.8 - 0.15);
                                let l = (x * x + y * y + z * z).sqrt().max(1e-4); [x / l, y / l, z / l] },
                    fill_col: { let s = c3(lsky); [s[0] * 0.55, s[1] * 0.55, s[2] * 0.55] },
                    ..Default::default()
                };
                p.add(crate::glow_scene::callback(rect, u, verts));
                // NAVIGATION CURSOR — where in the WORLD the pointer lands, not where it sits on
                // the glass. Same ray as the touch response, every hovered frame; the ring has a
                // FIXED WORLD radius, so its pixel size is a depth cue (near floor = wide, far
                // wall = small) — deliberately the inverse of the constant-subtended-angle rule
                // physical-ui-findings.md sets for CONTROLS: a control must stay pressable at any
                // distance, a cursor must SHOW distance. Hidden while anything drags (orbiting
                // reads as camera movement, not pointing). Drawn after the GL callback so it sits
                // on the rendered world.
                if let Some(hp) = ui.ctx().pointer_hover_pos() {
                    if rect.contains(hp) && !ui.input(|i| i.pointer.is_decidedly_dragging()) {
                        let uv = [(hp.x - rect.left()) / rect.width().max(1.0),
                                  1.0 - (hp.y - rect.top()) / rect.height().max(1.0)];
                        let (o, d) = cam.ray_uv(uv);
                        let hit = if d[1] < -1e-4 {
                            let th = (-o[1] / d[1]).clamp(0.5, 80.0);
                            [o[0] + d[0] * th, o[1] + d[1] * th, o[2] + d[2] * th]
                        } else {
                            [o[0] + d[0] * view_cam.dist, o[1] + d[1] * view_cam.dist, o[2] + d[2] * view_cam.dist]
                        };
                        let hv = crate::spatial::Vec3::new(hit[0], hit[1], hit[2]);
                        if let Some((sx, sy, _)) = view_cam.project(hv, cx, cy, scale) {
                            const CURSOR_R: f32 = 0.14; // world units
                            // pixel radius from a projected world-offset point (any horizontal axis works
                            // for a screen-space circle; the ring is a marker, not a floor decal)
                            let px = view_cam.project(crate::spatial::Vec3::new(hit[0] + CURSOR_R, hit[1], hit[2]), cx, cy, scale)
                                .map(|(ex, ey, _)| ((ex - sx).powi(2) + (ey - sy).powi(2)).sqrt())
                                .unwrap_or(4.0).clamp(2.0, 28.0);
                            let acc = crate::theme::active().amber; // the palette's accent family
                            let pulse = 150 + (40.0 * (t * 2.2).sin()) as i16;
                            let col = egui::Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), pulse.clamp(90, 200) as u8);
                            let c = egui::pos2(sx, sy);
                            p.circle_stroke(c, px, egui::Stroke::new(1.4_f32, col));
                            p.circle_filled(c, 1.6, col);
                        }
                    }
                }
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(40)); // strands ripple + runes glow
        }

        // For the pearl/fiber-optic scene the 3D light-cords + glossy pearls ARE the graph, so the flat
        // 2D edges and opaque dots below are suppressed to thin rings — they'd otherwise mask the bodies.
        let pearl_scene = theme::active().name == "Atlantis";
        // edges (dim if either endpoint is pruned)
        if !pearl_scene {
            for nd in &g.nodes {
                if let (Some(pid), Some((sx, sy)), Some((px, py))) = (nd.parent, screen(nd.id), nd.parent.and_then(screen)) {
                    let dim = self.pruned.contains(&nd.id) || self.pruned.contains(&pid);
                    let col = if dim { theme::LINE().linear_multiply(0.4) } else { theme::LINE() };
                    p.line_segment([egui::pos2(px, py), egui::pos2(sx, sy)], egui::Stroke::new(1.0_f32, col));
                }
            }
        }
        // nodes: color by section, size by depth × token "heat" (hot turns fatter), pruned = hollow
        let near = proj.iter().map(|q| q.3).fold(f32::MAX, f32::min);
        let far = proj.iter().map(|q| q.3).fold(0.0_f32, f32::max);
        let span = (far - near).max(0.001);
        let time = ui.input(|i| i.time) as f32;
        let acc = theme::accent();
        let _acc_a = |a: u8| egui::Color32::from_rgba_unmultiplied(acc.r(), acc.g(), acc.b(), a); // (ripple rings removed)
        // Node captions are collected during the draw and placed afterwards, so
        // they can be de-collided against each other. See the push site below.
        let mut label_wants: Vec<(crate::graph_labels::Want,)> = Vec::new();
        let mut label_items: Vec<(String, egui::FontId, egui::Color32)> = Vec::new();
        for &(id, sx, sy, depth) in &proj {
            let nd = &g.nodes[id];
            let t = ((depth - near) / span).clamp(0.0, 1.0); // 0 near, 1 far
            let hot = (nd.tokens as f32 / max_tok).clamp(0.0, 1.0);
            let radius = ((6.5 - 4.0 * t) * (1.0 + 0.7 * hot)).max(2.0);
            let sec = palette[seg_of[id] % palette.len()];
            let col = sec.linear_multiply(1.0 - 0.5 * t);
            let prn = self.pruned.contains(&id);
            let pt = egui::pos2(sx, sy);
            // (removed: the "gravitational air ripple" — 3 concentric circle_stroke rings that breathed outward to
            //  radius*7. For a near node those became huge room-sized skinny rings overlaying the floor across every
            //  scene. Significance is already carried by node size/glow; the rings read as stray geometry.)
            let _ = (hot, prn);
            if self.region.as_ref().map_or(false, |r| r.contains(&id)) { p.circle_stroke(pt, radius + 5.0, egui::Stroke::new(1.6_f32, egui::Color32::from_rgb(0xc9, 0x8f, 0xff))); } // :best region
            if self.walk_focus == Some(id) {
                // pulsing double halo so a voice/keyboard-focused node reads at a glance even mid-scene: an
                // outer ring that breathes in radius + a steady inner ring. (Was a single static ring.)
                let pulse = 0.5 + 0.5 * (time * 3.0 + id as f32).sin();
                p.circle_stroke(pt, radius + 3.0, egui::Stroke::new(2.2_f32, theme::AMBER()));
                p.circle_stroke(pt, radius + 5.5 + 3.5 * pulse, egui::Stroke::new(1.6_f32, theme::AMBER().linear_multiply(0.35 + 0.4 * (1.0 - pulse))));
            }
            if self.branch_from == Some(id) { p.circle_stroke(pt, radius + 6.5, egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(0xc9, 0x8f, 0xff))); }
            if self.active_leaf == Some(id) { p.circle_stroke(pt, radius + 2.0, egui::Stroke::new(1.5_f32, theme::CYAN())); } // live tip
            if prn {
                p.circle_stroke(pt, radius, egui::Stroke::new(1.0_f32, col.linear_multiply(0.5)));
            } else if pearl_scene {
                // outline only — let the 3D glossy pearl behind it show through
                p.circle_stroke(pt, radius, egui::Stroke::new(1.0_f32, col.linear_multiply(0.7)));
                if nd.turn == crate::convograph::Turn::User { p.circle_stroke(pt, radius + 1.5, egui::Stroke::new(1.0_f32, theme::CYAN().linear_multiply(0.6))); }
            } else if nd.kind.is_structural() {
                // FAN-OUT / MERGE: a wide, flat bar rather than a blob. It reads as a junction — the
                // point where the timeline splits or rejoins — instead of as one more thing that
                // happened, which is how three alternatives came to look like three replies.
                let h = radius * 0.45;
                let r = egui::Rect::from_center_size(pt, egui::vec2(radius * 3.0, h * 2.0));
                p.rect_filled(r, theme::r(2), theme::VIOLET());
                if radius > 5.0 {
                    p.text(pt, egui::Align2::CENTER_CENTER, nd.kind.glyph(),
                        egui::FontId::proportional((radius * 1.1).min(13.0)), theme::INK());
                }
            } else if nd.kind.is_narration() {
                // LIAISON: a hollow ring. Nobody said it and nothing was produced, so it should not
                // carry the visual weight of a turn — but it must remain findable, hence a ring rather
                // than a fainter dot, which would read as a quiet message.
                p.circle_stroke(pt, radius * 0.85, egui::Stroke::new(1.5_f32, theme::CYAN()));
            } else if nd.kind.is_artifact() {
                // ARTIFACTS ARE SQUARES; conversation turns are circles. Shape is the fastest visual
                // channel for "the conversation PRODUCED this" vs "someone SAID this", and unlike a hue
                // difference it survives greyscale, projection and colour-blindness. The glyph badge
                // (⑂ patch, 🗎 file, 🖼 image, 🎬 shot) then says which kind without a legend.
                let h = radius * 0.9;
                let r = egui::Rect::from_center_size(pt, egui::vec2(h * 2.0, h * 2.0));
                p.rect_filled(r, theme::r(2), col);
                p.rect_stroke(r, theme::r(2), egui::Stroke::new(1.0_f32, theme::INK().linear_multiply(0.6)),
                    egui::StrokeKind::Middle);
                let gl = nd.kind.glyph();
                if !gl.is_empty() && radius > 5.0 {
                    p.text(pt, egui::Align2::CENTER_CENTER, gl,
                        egui::FontId::proportional((radius * 1.1).min(14.0)), theme::INK());
                }
            } else {
                p.circle_filled(pt, radius, col);
                if nd.turn == crate::convograph::Turn::User { p.circle_stroke(pt, radius + 1.0, egui::Stroke::new(1.0_f32, theme::INK().linear_multiply(0.5))); }
            }
            if t < 0.42 && !prn {
                // hand off to the 3D resonance text up close: the 2D caption lives in a mid band, fading in
                // around the approach and fading back out as the pearl's own text resolves (t→0).
                let label_fade = ((t - 0.12) / 0.10).clamp(0.0, 1.0) * (1.0 - ((t - 0.30) / 0.12).clamp(0.0, 1.0));
                if label_fade > 0.02 {
                    // COLLECTED, not painted here. A fixed offset per node is what
                    // made these overlap into illegibility — see graph_labels.
                    // Measuring now and placing after the loop is the only order
                    // that works: you cannot avoid a collision you have not
                    // measured, and you cannot measure a label you have not laid
                    // out.
                    let txt = format!("{}t · {}", nd.tokens,
                        crate::nodelabel::elide_words(&nd.preview, 22));
                    let font = egui::FontId::proportional(11.0);
                    // `painter().layout_no_wrap` is this codebase's measuring
                    // idiom (theme.rs uses it in three places); `ui.fonts(..)`
                    // hands out a shared reference and cannot lay out.
                    let galley = p.layout_no_wrap(txt.clone(), font.clone(), egui::Color32::WHITE);
                    let base = if t < 0.2 { theme::INK() } else { theme::DIM() };
                    label_wants.push((
                        crate::graph_labels::Want {
                            id: label_items.len(),
                            x: sx + radius, y: sy,
                            w: galley.size().x, h: galley.size().y,
                            // The label the user is looking at survives a
                            // contested spot. Then brightness: one fading OUT is
                            // a better thing to drop than one fading in, and
                            // `label_fade` already encodes exactly that.
                            priority: if self.walk_focus == Some(id) { 10.0 } else { label_fade },
                        },
                        ));
                    label_items.push((txt, font, base.gamma_multiply(label_fade)));
                }
            }
        }
        // PLACE THE CAPTIONS, now that every one has been measured.
        //
        // Before this they were painted at a fixed offset from their node, so a
        // cluster produced the overlapping run visible in the field screenshot
        // ("14t · create a rust crate th" on top of "221t · rust use
        // std::colle"). `place` walks a fixed ladder of candidate offsets and
        // drops what will not fit rather than stacking it — three readable
        // captions and an honest "+7" beats ten illegible ones.
        {
            let wants: Vec<crate::graph_labels::Want> = label_wants.iter().map(|w| w.0).collect();
            let b = crate::graph_labels::Box2 {
                x: rect.left(), y: rect.top(), w: rect.width(), h: rect.height(),
            };
            let (placed, dropped) = crate::graph_labels::place(&wants, b, 2.0);
            for pl in &placed {
                let (txt, font, col) = &label_items[pl.id];
                // A flipped label sits LEFT of its node, so it must be
                // right-aligned or it runs back over the dot it belongs to.
                let (anchor, x) = if pl.flipped {
                    (egui::Align2::RIGHT_CENTER, pl.at.x + pl.at.w)
                } else {
                    (egui::Align2::LEFT_CENTER, pl.at.x)
                };
                p.text(egui::pos2(x, pl.at.y + pl.at.h * 0.5), anchor, txt, font.clone(), *col);
            }
            if dropped > 0 {
                // Said, not hidden. A caption silently missing reads as a bug in
                // the graph; a count reads as a full view.
                p.text(egui::pos2(rect.right() - 6.0, rect.top() + 6.0), egui::Align2::RIGHT_TOP,
                    format!("+{dropped} more"), egui::FontId::proportional(10.0),
                    theme::DIM());
            }
        }
        // glowing identicon runes inscribed on each near stone — the node's short-id, encoded + emissive
        {
            let tt = ui.input(|i| i.time) as f32;
            let grid = 5usize;
            for &(id, sx, sy, depth) in &proj {
                let frac = ((depth - near) / span).clamp(0.0, 1.0);
                if frac > 0.5 { continue; }                 // only legible on the near stones
                let half = (0.26 * scale / depth) * 0.85;   // rune box ≈ the stone's projected size
                if half < 6.0 { continue; }
                // quiet the runes as the camera closes in: they HAND OFF to the 3D resonance text up close.
                // Strongest at mid-approach, gone very near (text takes over) and gone far.
                let approach = ((frac - 0.05) / 0.16).clamp(0.0, 1.0); // 0 when very near → runes quiet
                if approach < 0.03 { continue; }
                let cell = (half * 2.0) / grid as f32;
                let pulse = 0.55 + 0.45 * (tt * 1.7 + id as f32 * 0.6).sin();
                let glow = (if self.active_leaf == Some(id) { theme::CYAN() } else { theme::AMBER() })
                    .gamma_multiply(approach * (1.0 - frac) * pulse);
                let (ox, oy) = (sx - half, sy - half);
                for (cxx, cyy) in crate::mesh3d::rune_cells(crate::vimmode::short_id_seed(id), grid) {
                    let p0 = egui::pos2(ox + cxx as f32 * cell + cell * 0.1, oy + cyy as f32 * cell + cell * 0.1);
                    p.rect_filled(egui::Rect::from_min_size(p0, egui::vec2(cell * 0.8, cell * 0.8)), theme::r(1), glow);
                }
            }
        }
        // DEBUG (LOGSCOPE_DEBUG_LABELS): label the NEAREST 10 scene meshes on-screen — mesh index + its max
        // dimension in world units — so any element (e.g. a stray ring) can be identified by eye and cross-
        // referenced with the /tmp/scene_labels_<theme>.txt dump. Rebuilds the env once/frame; debug-only.
        if std::env::var("LOGSCOPE_DEBUG_LABELS").is_ok() {
            let mut labels: Vec<(usize, f32, f32, f32, f32)> = Vec::new(); // (idx, sx, sy, depth, max_dim)
            crate::scene3d::with_env(theme::active().name, ui.input(|i| i.time) as f32, |meshes| {
                for (i, m) in meshes.iter().enumerate() {
                    if m.verts.is_empty() { continue; }
                    let (mut sxw, mut syw, mut szw) = (0.0f32, 0.0f32, 0.0f32);
                    let (mut lo, mut hi) = (m.verts[0], m.verts[0]);
                    for v in &m.verts {
                        sxw += v.x; syw += v.y; szw += v.z;
                        lo.x = lo.x.min(v.x); lo.y = lo.y.min(v.y); lo.z = lo.z.min(v.z);
                        hi.x = hi.x.max(v.x); hi.y = hi.y.max(v.y); hi.z = hi.z.max(v.z);
                    }
                    let nn = m.verts.len() as f32;
                    let c = crate::spatial::Vec3::new(sxw / nn, syw / nn, szw / nn);
                    if let Some((px, py, depth)) = view_cam.project(c, cx, cy, scale) {
                        let md = (hi.x - lo.x).max(hi.y - lo.y).max(hi.z - lo.z);
                        labels.push((i, px, py, depth, md));
                    }
                }
            });
            labels.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
            labels.truncate(10);
            for (i, px, py, _d, md) in labels {
                let pt = egui::pos2(px, py);
                p.circle_stroke(pt, 5.0, egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(0xff, 0x4d, 0x6d)));
                let lab = format!("#{} {:.0}u", i, md);
                let fid = egui::FontId::monospace(12.0);
                p.text(pt + egui::vec2(8.0, -5.0), egui::Align2::LEFT_CENTER, &lab, fid.clone(), egui::Color32::BLACK);
                p.text(pt + egui::vec2(7.0, -6.0), egui::Align2::LEFT_CENTER, &lab, fid, egui::Color32::from_rgb(0xff, 0xe0, 0x55));
            }
        }

        // Node picking, via `scenepick` — a pure, tested hit-tester (accessibility floor so a distant
        // small node stays hittable, real-hit-vs-padding-only tie-breaking) that already existed and
        // was never called anywhere in the crate. It replaces a fixed 26px-radius nearest-in-screen-
        // space search that had no accessibility floor at all and no notion of depth ties.
        let touch_target = uikit_egui::adaptive::Adaptive::detect(ui.ctx()).min_touch_target;
        let pick_targets: Vec<crate::scenepick::Target> = proj.iter()
            .map(|&(id, _, _, _)| crate::scenepick::Target { id: id as u32, at: pos[id], radius: 0.26, enabled: true })
            .collect();
        // HOVER — the node under the pointer, every frame (not just on click), so it can lift/glow
        // before being pressed; the physical-UI premise this whole family of fixes is chasing is that
        // an object answers *attention*, not only a click.
        self.scene_hover = ui.ctx().pointer_hover_pos()
            .filter(|_| !ui.input(|i| i.pointer.is_decidedly_dragging()))
            .and_then(|hp| crate::scenepick::pick(&view_cam, &pick_targets, hp.x, hp.y, cx, cy, scale, touch_target))
            .map(|id| id as usize);
        if self.scene_hover.is_some() { ui.ctx().request_repaint(); }
        // PRESSED — same pick, gated on the primary button actually being held over the node. Using
        // the interact position (not the raw pointer position `scene_hover` uses) so this reflects
        // where the button went down, matching what `resp` itself is tracking for the eventual click.
        self.scene_pressed = ui.input(|i| i.pointer.primary_down())
            .then(|| resp.interact_pointer_pos())
            .flatten()
            .and_then(|pp| crate::scenepick::pick(&view_cam, &pick_targets, pp.x, pp.y, cx, cy, scale, touch_target))
            .map(|id| id as usize);

        // click → nearest node (focus it; opt out of auto-follow so it stays put)
        if resp.clicked() {
            if let Some(ptr) = resp.interact_pointer_pos() {
                let hit = crate::scenepick::pick(&view_cam, &pick_targets, ptr.x, ptr.y, cx, cy, scale, touch_target);
                if let Some(id) = hit.map(|id| id as usize) {
                    // glide along the graph's shortest path from the current focus to the clicked node
                    let from = self.walk_focus.unwrap_or(id);
                    let path = graph_path(&g, from, id);
                    if path.len() > 1 { self.cam_pan_path = path; self.cam_pan_t0 = Some(std::time::Instant::now()); }
                    else { self.orbit.target = pos[id]; }
                    self.walk_focus = Some(id); self.spatial_auto = false; ui.ctx().request_repaint();
                }
            }
        }

        // focused-node preview strip with prune/restore for that turn (skipped when used as a backdrop).
        // Floated as translucent glass anchored to the bottom of the scene — reserves no layout space.
        if !self.backdrop {
        let panel_w = ui.max_rect().width();
        egui::Area::new(egui::Id::new("scene_bottom_chrome"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -8.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
            ui.set_max_width((panel_w - 16.0).max(160.0));
            glass_frame().show(ui, |ui| {
            if let Some(id) = self.walk_focus.filter(|i| *i < self.msgs.len()) {
                // The turn TEXT now reads on the physical plaque in the scene; this glass strip is just the
                // per-turn action bar (prune / restore / branch) for the focused node.
                let who = if self.msgs[id].role == Role::User { "you" } else { "assistant" };
                let prn = self.pruned.contains(&id);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{who} · turn {}/{}", id + 1, n)).color(theme::AMBER()).small().strong());
                    if prn {
                        if ui.small_button("↩ Restore").clicked() { self.pruned.remove(&id); ui.ctx().request_repaint(); }
                        ui.label(RichText::new("· pruned, out of context").color(theme::DIM()).small());
                    } else if ui.small_button("✂ Prune").on_hover_text("Drop this turn from the model's context").clicked() {
                        self.pruned.insert(id); ui.ctx().request_repaint();
                    }
                    if self.branch_from == Some(id) {
                        ui.label(RichText::new("⑂ next message forks here").color(theme::CYAN()).small().strong());
                        if ui.small_button("✕").on_hover_text("Cancel branch").clicked() { self.branch_from = None; }
                    } else if ui.small_button("⑂ Branch here").on_hover_text("Reply as a side-thread off this turn — the next message you send forks here instead of continuing the tip").clicked() {
                        self.branch_from = Some(id); ui.ctx().request_repaint();
                    }
                });
            } else {
                ui.label(RichText::new("Click a node to read its turn here. ⊹ Overview reframes; ✂ prunes to free context.").color(theme::FAINT()).small());
            }
            }); });
        }
        send_now
    }
    /// The fly-through: `LOGSCOPE_FLY` / `LOGSCOPE_FLY_AUTO` walk the camera along the
    /// conversation path, facing the direction of travel.
    ///
    /// Lifted out of `spatial_ui`, where it sat between the node layout and the mesh draw —
    /// three unrelated things in a row, and this is the only one that moves the camera.
    ///
    /// Extracted AGAINST THE COMPILER rather than by eye: the parameters are the names rustc
    /// reported as unresolved, not the ones I guessed it needed. A block this long in the
    /// middle of a 2,700-line function has no honest way to be read for its dependencies.
    fn fly_through(&mut self, ui: &egui::Ui, n: usize, pos: &[crate::spatial::Vec3]) {
        // Fly-through (headless / smooth camera move): LOGSCOPE_FLY=s (0..1) travels the camera target ALONG
        // the conversation path (id order) and FACES the direction of travel, sitting just inside the palace —
        // so we move THROUGH the mind palace. LOGSCOPE_FLY_AUTO=speed animates s from the clock for a
        // continuous glide. The forward look is derived from the path tangent (see OrbitCam::project).
            let auto = self.fly_auto.or_else(|| std::env::var("LOGSCOPE_FLY_AUTO").ok().and_then(|v| v.trim().parse::<f32>().ok()));
            let manual = std::env::var("LOGSCOPE_FLY").ok().and_then(|v| v.trim().parse::<f32>().ok());
            let s_opt = if let Some(spd) = auto {
                // Variable-speed traversal: advance the strand position by dt scaled DOWN by the current node's
                // dwell weight, so longer messages linger and short ones pass quicker (bounded, see tour_dwell).
                // Falls back to the old time-linear mapping if dwell weights aren't computed yet.
                if !self.tour_dwell.is_empty() && n > 1 {
                    let dt = ui.input(|i| i.stable_dt).min(0.05);
                    let cur = (self.tour_s.clamp(0.0, 0.999) * (n - 1) as f32).round() as usize;
                    let dwell = self.tour_dwell.get(cur).copied().unwrap_or(1.0);
                    self.tour_s = (self.tour_s + dt * spd * 0.02 / dwell).fract();
                    Some(self.tour_s)
                } else {
                    let tt = ui.input(|i| i.time) as f32;
                    Some(((tt * spd * 0.015).fract() + 1.0).fract())
                }
            } else { manual };
            if let (Some(s), true) = (s_opt, n > 1) {
                use crate::spatial::Vec3 as V3;
                let s = s.clamp(0.0, 0.999);
                let seg = s * (n - 1) as f32;
                let i0 = (seg.floor() as usize).min(n - 1);
                let i1 = (i0 + 1).min(n - 1);
                let fr = seg - i0 as f32;
                // Catmull-Rom through the 4 surrounding nodes → a smooth curve, not a polyline of kinks
                let cr = |p0: V3, p1: V3, p2: V3, p3: V3, u: f32| {
                    let u2 = u * u; let u3 = u2 * u;
                    let f = |a: f32, b: f32, c: f32, d: f32|
                        0.5 * ((2.0 * b) + (-a + c) * u + (2.0 * a - 5.0 * b + 4.0 * c - d) * u2 + (-a + 3.0 * b - 3.0 * c + d) * u3);
                    V3::new(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y), f(p0.z, p1.z, p2.z, p3.z))
                };
                let pm = pos[i0.saturating_sub(1)]; let p1 = pos[i0]; let p2 = pos[i1]; let p3 = pos[(i1 + 1).min(n - 1)];
                let here = cr(pm, p1, p2, p3, fr);
                let look = cr(pm, p1, p2, p3, (fr + 0.34).min(1.0)); // look well ahead → a SMOOTH tangent that doesn't whip at every node
                let (tx, ty, tz) = (look.x - here.x, look.y - here.y, look.z - here.z);
                let tl = (tx * tx + ty * ty + tz * tz).sqrt().max(1e-3);
                let (txn, tyn, tzn) = (tx / tl, ty / tl, tz / tl);
                let base_pitch = ((-tyn).asin().clamp(-1.2, 1.2)) + 0.12; // downward bias to see floor/pearls
                let base_yaw = txn.atan2(-tzn);
                let base_dist = std::env::var("LOGSCOPE_FLY_DIST").ok().and_then(|d| d.trim().parse().ok()).unwrap_or(5.2);
                // ── CINEMATIC DIRECTOR ── a shot state machine fed the graph's AABB (centroid + bounding radius)
                // and the current strand point. It cycles slowly through filmic shots — travelling along the strand,
                // pulling back to ESTABLISH the whole graph (the re-frame on arrival), slow ORBIT arcs, DUTCH tilts,
                // and close PUSH-IN passes — with slow eases so the camera glides like a movie rather than snapping.
                let now = ui.input(|i| i.time) as f32;
                let cc = crate::spatial::centroid(&pos);
                let fitd = crate::spatial::fit_distance(crate::spatial::bounding_radius(&pos, cc));
                let rnd = |seed: f32| ((seed * 12.9898).sin() * 43758.5453).fract().abs();
                if now >= self.cam_shot_until {
                    let h = rnd(now);
                    self.cam_shot = match self.cam_shot {                         // arrival → often ESTABLISH (re-frame graph)
                        0 => if h < 0.5 { 1 } else if h < 0.75 { 2 } else { 3 },  // travel → establish / orbit / dutch
                        1 => if h < 0.6 { 0 } else { 2 },                         // establish → travel / orbit
                        2 => if h < 0.55 { 0 } else if h < 0.8 { 1 } else { 4 },  // orbit → travel / establish / push
                        3 => 0,                                                   // dutch → travel
                        _ => if h < 0.6 { 0 } else { 1 },                         // push → travel / establish
                    };
                    self.cam_shot_until = now + 11.0 + h * 8.0;                   // 11–19 s dwell: fewer cuts, slower film pacing
                }
                self.cam_orbit_phase += 0.0026;                                  // slow continuous drift for orbit/dutch
                let (mut want_target, mut want_yaw, mut want_pitch, mut want_dist, mut want_roll, mut k) =
                    (here, base_yaw, base_pitch, base_dist, 0.0f32, 0.02f32);     // default: slow travel along the strand
                match self.cam_shot {
                    1 => { want_target = cc; want_dist = fitd; want_pitch = 0.42; want_yaw = base_yaw + (self.cam_orbit_phase * 0.4).sin() * 0.5; k = 0.014; } // ESTABLISH
                    2 => { want_target = here; want_dist = (fitd * 0.55).clamp(7.0, 40.0); want_pitch = 0.3; want_yaw = self.cam_orbit_phase * 0.8; k = 0.018; } // ORBIT arc
                    3 => { want_roll = (self.cam_orbit_phase * 0.5).sin() * 0.26; k = 0.022; }                                                                   // DUTCH tilt
                    4 => { want_dist = 4.4; want_pitch = base_pitch * 0.7; k = 0.026; }                                                                          // PUSH-IN passthrough (less extreme)
                    _ => {}
                }
                // GUARD: a collapsed graph (bounding_radius→0) or a gigantic one can make fit_distance return
                // NaN/inf or an absurd distance; a degenerate spline point can make want_target non-finite. Any
                // of these propagate into orbit.dist/target → a NaN eye → NaN projection → a BLANK/broken frame
                // (this is the "starting the tour breaks the render" bug). Clamp them to sane, finite values here.
                if !want_dist.is_finite() { want_dist = base_dist; }
                want_dist = want_dist.clamp(2.0, 240.0);
                if !(want_target.x.is_finite() && want_target.y.is_finite() && want_target.z.is_finite()) { want_target = here; }
                if !want_yaw.is_finite() { want_yaw = base_yaw; }
                if !want_pitch.is_finite() { want_pitch = base_pitch; }
                self.orbit.target.x += (want_target.x - self.orbit.target.x) * k;
                self.orbit.target.y += (want_target.y - self.orbit.target.y) * k;
                self.orbit.target.z += (want_target.z - self.orbit.target.z) * k;
                let mut dyaw = want_yaw - self.orbit.yaw;
                while dyaw > std::f32::consts::PI { dyaw -= std::f32::consts::TAU; }
                while dyaw < -std::f32::consts::PI { dyaw += std::f32::consts::TAU; }
                self.orbit.yaw += dyaw * k;
                self.orbit.pitch += (want_pitch - self.orbit.pitch) * k;
                self.orbit.dist += (want_dist - self.orbit.dist) * k;
                self.orbit.roll += (want_roll - self.orbit.roll) * (k * 0.8);    // ease the Dutch tilt in/out
                // final belt-and-braces: never let the eased orbit carry a non-finite value into projection
                if !self.orbit.dist.is_finite() { self.orbit.dist = base_dist; }
                self.orbit.dist = self.orbit.dist.clamp(2.0, 240.0);
                if !self.orbit.yaw.is_finite() { self.orbit.yaw = base_yaw; }
                if !self.orbit.pitch.is_finite() { self.orbit.pitch = base_pitch; }
                if !(self.orbit.target.x.is_finite() && self.orbit.target.y.is_finite() && self.orbit.target.z.is_finite()) { self.orbit.target = here; }

                // CAMERA BOUNDING-SPHERE EVASION: keep the eye OUT of opaque solids (columns / central plinth)
                // and INSIDE the perimeter wall, using the colliders the env generator emitted from the same
                // EnvParams that built the hall. We shift the whole camera (target+eye) by the push so the look
                // direction is preserved — no more flying blind into a stone column or the plinth.
                let (cyf, syf) = (self.orbit.yaw.cos(), self.orbit.yaw.sin());
                let (cpf, spf) = (self.orbit.pitch.cos(), self.orbit.pitch.sin());
                let dd0 = self.orbit.dist;
                let eye = V3::new(self.orbit.target.x + dd0 * (-cpf * syf),
                                  self.orbit.target.y + dd0 * spf,
                                  self.orbit.target.z + dd0 * (cpf * cyf));
                let cam_r = 0.85f32; // camera collision volume (bounding-sphere radius)
                // openings (doors / hallway mouths) where the camera may cross the perimeter Bound
                let gates: Vec<(f32, f32)> = crate::scene3d::env_colliders().iter().filter_map(|c| match c {
                    crate::scene3d::Collider::Gate { ang, half } => Some((*ang, *half)), _ => None,
                }).collect();
                // floor/ceiling envelope from the solids, so the eye rides BETWEEN floor and dome (no diving
                // through the floor or punching the roof while it follows the strand).
                let (mut floor_y, mut ceil_y) = (f32::MAX, f32::MIN);
                // Horizontal palace AABB from the same colliders (Pillars cover hallway + wing geometry, Bound the
                // rotunda perimeter) → a hard OUTER bound for the fly, below.
                let (mut ax0, mut az0, mut ax1, mut az1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for c in crate::scene3d::env_colliders() {
                    match c {
                        crate::scene3d::Collider::Pillar { x, z, r, y0, y1 } => {
                            floor_y = floor_y.min(y0); ceil_y = ceil_y.max(y1);
                            ax0 = ax0.min(x - r); ax1 = ax1.max(x + r); az0 = az0.min(z - r); az1 = az1.max(z + r);
                        }
                        crate::scene3d::Collider::Bound { r } => {
                            ax0 = ax0.min(-r); ax1 = ax1.max(r); az0 = az0.min(-r); az1 = az1.max(r);
                        }
                        _ => {}
                    }
                }
                let (mut px, mut py, mut pz) = (0.0f32, 0.0f32, 0.0f32);
                // a few relaxation passes so the eye settles in the GAP between solids (one pass under-resolves
                // when it's wedged between two columns), instead of tunnelling through a wall.
                for _ in 0..3 {
                    for col in crate::scene3d::env_colliders() {
                        match col {
                            crate::scene3d::Collider::Pillar { x, z, r, y0, y1 } => {
                                let ey = eye.y + py;
                                if ey > y0 - cam_r && ey < y1 + cam_r {
                                    let (dx, dz) = (eye.x + px - x, eye.z + pz - z);
                                    let dist = (dx * dx + dz * dz).sqrt();
                                    let rr = r + cam_r;
                                    if dist < rr {
                                        if dist > 1e-3 { let f = (rr - dist) / dist; px += dx * f; pz += dz * f; }
                                        else { px += rr; }
                                    }
                                }
                            }
                            crate::scene3d::Collider::Bound { r } => {
                                let (ex, ez) = (eye.x + px, eye.z + pz);
                                let dist = (ex * ex + ez * ez).sqrt();
                                let rr = r - cam_r;
                                if dist > rr && dist > 1e-3 {
                                    // let the camera slip OUT through a door / hallway mouth; otherwise hold it in
                                    let ang = ez.atan2(ex);
                                    let open = gates.iter().any(|&(ga, half)| { let d = (ang - ga).abs(); d.min(std::f32::consts::TAU - d) < half });
                                    if !open { let f = (dist - rr) / dist; px -= ex * f; pz -= ez * f; }
                                }
                            }
                            crate::scene3d::Collider::Gate { .. } => {} // handled above (pre-collected)
                        }
                    }
                }
                if floor_y.is_finite() {
                    let ey = eye.y + py;
                    let (lo, hi) = (floor_y + 1.2, ceil_y - 0.6);
                    if ey < lo { py += lo - ey; }
                    else if hi > lo && ey > hi { py += hi - ey; }
                }
                // COLLIDER-INDEPENDENT HEIGHT CAP: even when a scene emits no floor/ceiling colliders (so the
                // clamp above is skipped) or the dome is very tall, never let the eye climb more than a bounded
                // vantage above/below the graph itself. Without this, a big-graph ESTABLISH pull-back sends the
                // eye straight into the sky — the "start tour flies up very high" bug. Generous (2.5× the
                // bounding radius) so it still frames the whole palace from above, just not from orbit.
                {
                    let br = crate::spatial::bounding_radius(&pos, cc).max(4.0);
                    let ey = eye.y + py;
                    let hi = cc.y + br * 2.5 + 6.0;
                    let lo = cc.y - br * 2.5 - 6.0;
                    if ey > hi { py += hi - ey; } else if ey < lo { py += lo - ey; }
                }
                // HARD OUTER BOUND: clamp the eye into the palace AABB (+margin) so bounded traversal holds even
                // when the fly slips out a gate down a hall — it stops at the hall's end instead of sailing into
                // open ocean (pass-63 overshoot). Bound = union of the node extent (so the eye can always reach
                // every strand node and pull back to ESTABLISH them) and the collider solids (walls/wings), so the
                // clamp never fights the spline; it only catches what would leave the palace.
                let (mut bx0, mut bz0, mut bx1, mut bz1) = (ax0, az0, ax1, az1);
                for q in pos { bx0 = bx0.min(q.x); bx1 = bx1.max(q.x); bz0 = bz0.min(q.z); bz1 = bz1.max(q.z); }
                // Skip the outer clamp during the ESTABLISH (1) and ORBIT (2) shots — those DELIBERATELY pull
                // the eye far outside the graph extent to frame the whole palace. Clamping them jammed the eye
                // onto the graph edge, so the scene fell entirely out of view ("starting tour blanks the
                // scene"). The close travel/dutch/push shots still get the clamp, catching a slip out a hallway
                // into open ocean (the pass-63 overshoot it was added for).
                let framing_shot = matches!(self.cam_shot, 1 | 2);
                if bx0.is_finite() && !framing_shot {
                    let m = 4.0;
                    let ex = eye.x + px;
                    if ex < bx0 - m { px += (bx0 - m) - ex; } else if ex > bx1 + m { px += (bx1 + m) - ex; }
                    let ez = eye.z + pz;
                    if ez < bz0 - m { pz += (bz0 - m) - ez; } else if ez > bz1 + m { pz += (bz1 + m) - ez; }
                }
                // EASE the evasion offset toward the freshly-computed push so the camera SLIDES around columns and
                // along walls instead of snapping (the snap is what read as "ricochet"). The smoothed offset is
                // what shifts the eye+target, preserving look direction.
                let target_push = crate::spatial::Vec3::new(px, py, pz);
                self.fly_push.x += (target_push.x - self.fly_push.x) * 0.18;
                self.fly_push.y += (target_push.y - self.fly_push.y) * 0.18;
                self.fly_push.z += (target_push.z - self.fly_push.z) * 0.18;
                self.orbit.target.x += self.fly_push.x; self.orbit.target.y += self.fly_push.y; self.orbit.target.z += self.fly_push.z;

                self.spatial_auto = false; self.spatial_zoomed = true;
            }
    }

}
