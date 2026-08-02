//! Duplicates & roll-up — the last unported Qt flow (QML Settings.qml's rollup card): scan the
//! configured roots for duplicate checkouts of the same project, then for a chosen (older, newer)
//! pair either fast-forward roll-up (safe, `--ff-only`) or run a merge **exploration** — cockpit
//! builds a throwaway workspace with one `gm-<id>` branch per strategy — preview what each
//! strategy would change (`flow::merge_preview`: the action the port doc listed as remaining),
//! and land the chosen one as a new branch in the real repo.
//!
//! All blocking calls go through the shared [`crate::task::Task`] (one job at a time, same as the
//! Qt `Job`). Task carries a single `String`, so structured results ride a tiny `\x1E`/`\x1F`/
//! newline wire format — the encode/decode pairs are pure and unit-tested, and every closure
//! encodes exactly one of them, tagged by [`Pending`] so the poll site knows how to decode.

use crate::app::App;
use crate::task::Task;

/// Which decode the in-flight task's result needs.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Pending {
    #[default]
    None,
    Scan,
    Rollup,
    RollupApply,
    Explore,
    Preview,
    Land,
}

#[derive(Clone, Default)]
pub struct Group {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Default)]
pub struct RollupView {
    pub relation: String,
    pub can_ff: bool,
    pub behind: usize,
    pub ahead: usize,
    pub incoming: Vec<(String, String)>,
    pub files: Vec<(String, String)>,
}

#[derive(Clone, Default)]
pub struct TrialView {
    pub id: String,
    pub label: String,
    pub ok: bool,
    pub conflicts: usize,
    pub commits: usize,
    pub summary: String,
    pub branch: String,
}

#[derive(Clone, Default)]
pub struct ExplView {
    pub workspace: String,
    pub target_branch: String,
    pub trials: Vec<TrialView>,
}

#[derive(Clone, Default)]
pub struct PreviewView {
    pub trial_id: String,
    pub branch: String,
    pub shortstat: String,
    pub commits: Vec<(String, String)>,
    pub files: Vec<(String, String)>,
}

#[derive(Default)]
pub struct State {
    pub scanned: bool,
    pub groups: Vec<Group>,
    /// The chosen pair: roll the OLDER copy forward / merge it INTO the newer.
    pub older: String,
    pub newer: String,
    pub rollup: Option<RollupView>,
    pub expl: Option<ExplView>,
    pub preview: Option<PreviewView>,
    pub note: String,
    pub pending: Pending,
    /// How the chosen pair actually relate: same / ahead / behind / diverged /
    /// unrelated, with the commit counts each way.
    ///
    /// The page let the user LABEL one copy "older" and one "newer" with
    /// nothing checking whether that was true — and then offered a
    /// fast-forward, which only succeeds when one really is an ancestor of the
    /// other. Getting it backwards produced a git error the page reported
    /// verbatim, which is a poor way to learn that you picked the wrong side.
    pub relation: Option<(String, usize, usize)>,
}

/// Can `older -> newer` fast-forward?
///
/// Only when the newer copy contains all of the older one's history. "diverged"
/// and "unrelated" cannot, and neither can a pair chosen the wrong way round —
/// which is the common mistake, since the labels are assigned by hand.
pub fn can_fast_forward(relation: &str) -> bool {
    // "behind" means A (the one we called older) is behind B — exactly the
    // situation a fast-forward resolves.
    matches!(relation, "behind" | "same")
}

/// What to tell the user about a relation, in terms of the action they are
/// about to take rather than git's vocabulary.
pub fn relation_hint(relation: &str, a_ahead: usize, b_ahead: usize) -> String {
    match relation {
        "same" => "identical — nothing to roll up".into(),
        "behind" => format!("older is {b_ahead} commits behind — a fast-forward will work"),
        "ahead" => format!("these are the wrong way round: the copy you marked older is \
                            {a_ahead} commits AHEAD"),
        "diverged" => format!("diverged: {a_ahead} and {b_ahead} commits each way — \
                              a fast-forward cannot work, explore a merge instead"),
        _ => "unrelated histories — not two copies of one project".into(),
    }
}

// ── wire codec (Task carries one String; ok/err + records) ───────────────────

const OK: &str = "ok\x1e";
const ERR: &str = "err\x1e";

fn enc_pairs(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(a, b)| format!("{a}\x1f{b}")).collect::<Vec<_>>().join("\x1d")
}
fn dec_pairs(s: &str) -> Vec<(String, String)> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split('\x1d')
        .filter_map(|r| r.split_once('\x1f').map(|(a, b)| (a.to_string(), b.to_string())))
        .collect()
}

pub fn enc_groups(gs: &[(String, Vec<String>)]) -> String {
    let body: Vec<String> = gs.iter().map(|(n, ps)| format!("{n}\x1f{}", ps.join("\x1f"))).collect();
    format!("{OK}{}", body.join("\n"))
}
fn dec_groups(s: &str) -> Vec<Group> {
    s.lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut it = l.split('\x1f');
            Group { name: it.next().unwrap_or("").to_string(), paths: it.map(str::to_string).collect() }
        })
        .collect()
}

pub fn enc_rollup(r: &cockpit::flow::RollupPreview) -> String {
    format!(
        "{OK}{}\x1f{}\x1f{}\x1f{}\n{}\n{}",
        r.relation,
        r.can_fast_forward as u8,
        r.behind,
        r.ahead,
        enc_pairs(&r.incoming),
        enc_pairs(&r.files)
    )
}
fn dec_rollup(s: &str) -> Option<RollupView> {
    let mut lines = s.splitn(3, '\n');
    let head = lines.next()?;
    let mut h = head.split('\x1f');
    Some(RollupView {
        relation: h.next()?.to_string(),
        can_ff: h.next()? == "1",
        behind: h.next()?.parse().ok()?,
        ahead: h.next()?.parse().ok()?,
        incoming: dec_pairs(lines.next().unwrap_or("")),
        files: dec_pairs(lines.next().unwrap_or("")),
    })
}

pub fn enc_expl(e: &cockpit::merge::Exploration) -> String {
    let trials: Vec<String> = e
        .trials
        .iter()
        .map(|t| {
            format!(
                "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
                t.id, t.label, t.ok as u8, t.conflicts, t.commits, t.branch,
                t.summary.replace('\n', " ")
            )
        })
        .collect();
    format!("{OK}{}\x1f{}\n{}", e.workspace.display(), e.target_branch, trials.join("\n"))
}
fn dec_expl(s: &str) -> Option<ExplView> {
    let mut lines = s.lines();
    let head = lines.next()?;
    let (ws, target) = head.split_once('\x1f')?;
    let trials = lines
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let p: Vec<&str> = l.splitn(7, '\x1f').collect();
            if p.len() < 7 {
                return None;
            }
            Some(TrialView {
                id: p[0].into(),
                label: p[1].into(),
                ok: p[2] == "1",
                conflicts: p[3].parse().ok()?,
                commits: p[4].parse().ok()?,
                branch: p[5].into(),
                summary: p[6].into(),
            })
        })
        .collect();
    Some(ExplView { workspace: ws.to_string(), target_branch: target.to_string(), trials })
}

pub fn enc_preview(p: &cockpit::flow::MergePreview) -> String {
    format!("{OK}{}\n{}\n{}", p.shortstat.replace('\n', " "), enc_pairs(&p.commits), enc_pairs(&p.files))
}
fn dec_preview(s: &str) -> Option<PreviewView> {
    let mut lines = s.splitn(3, '\n');
    Some(PreviewView {
        trial_id: String::new(),
        branch: String::new(),
        shortstat: lines.next()?.to_string(),
        commits: dec_pairs(lines.next().unwrap_or("")),
        files: dec_pairs(lines.next().unwrap_or("")),
    })
}

fn ok_body(result: &str) -> Result<&str, String> {
    if let Some(b) = result.strip_prefix(OK) {
        Ok(b)
    } else if let Some(e) = result.strip_prefix(ERR) {
        Err(e.to_string())
    } else {
        Err(format!("unrecognized task result: {result:.60}"))
    }
}

// ── task launchers ───────────────────────────────────────────────────────────

fn start(task: &Task, st: &mut State, kind: Pending, f: impl FnOnce() -> String + Send + 'static) {
    if task.run(f) {
        st.pending = kind;
        st.note.clear();
    } else {
        st.note = "a job is already running".into();
    }
}

// ── the card ─────────────────────────────────────────────────────────────────

pub fn card(app: &mut App, ui: &mut egui::Ui) {
    // poll the in-flight job (one at a time, per Task's contract)
    if app.dupes.pending != Pending::None {
        if let Some(res) = app.task.take_result() {
            let st = &mut app.dupes;
            let pending = std::mem::take(&mut st.pending);
            match ok_body(&res) {
                Err(e) => st.note = format!("\u{2717} {e}"),
                Ok(body) => match pending {
                    Pending::Scan => {
                        st.groups = dec_groups(body);
                        st.scanned = true;
                        st.note = format!("{} duplicate group(s)", st.groups.len());
                    }
                    Pending::Rollup => st.rollup = dec_rollup(body),
                    Pending::RollupApply => {
                        st.note = format!("\u{2713} {body}");
                        st.rollup = None;
                        st.scanned = false; // relations changed — rescan on next open
                    }
                    Pending::Explore => {
                        st.expl = dec_expl(body);
                        st.preview = None;
                    }
                    Pending::Preview => {
                        if let Some(mut pv) = dec_preview(body) {
                            // trial identity was stashed in the note while running
                            if let Some((id, br)) = st.note.split_once('\x1f') {
                                pv.trial_id = id.to_string();
                                pv.branch = br.to_string();
                            }
                            st.preview = Some(pv);
                        }
                        st.note.clear();
                    }
                    Pending::Land => {
                        st.note = format!("\u{2713} {body}");
                        st.expl = None;
                        st.preview = None;
                    }
                    Pending::None => {}
                },
            }
        } else {
            ui.spinner();
            ui.ctx().request_repaint();
        }
    }

    ui.heading("Duplicates & roll-up");
    ui.label(
        egui::RichText::new(
            "find duplicate checkouts across the scan roots; fast-forward a stale copy, or explore merge strategies and land one",
        )
        .weak()
        .small(),
    );

    if ui.button("\u{1F50D} scan for duplicates").clicked() {
        start(&app.task, &mut app.dupes, Pending::Scan, || {
            let gs: Vec<(String, Vec<String>)> =
                cockpit::flow::duplicates_all().into_iter().map(|g| (g.name, g.paths)).collect();
            enc_groups(&gs)
        });
    }

    let st = &mut app.dupes;
    if !st.note.is_empty() && !st.note.contains('\x1f') {
        ui.label(&st.note);
    }

    for g in st.groups.clone() {
        ui.separator();
        ui.label(egui::RichText::new(&g.name).strong());
        for p in &g.paths {
            ui.horizontal(|ui| {
                if ui.selectable_label(st.older == *p, "older").clicked() {
                    st.older = p.clone();
                    // A relation describes ONE pair; keeping it after a change
                    // shows the previous pair's verdict beside the new one.
                    st.relation = None;
                }
                if ui.selectable_label(st.newer == *p, "newer").clicked() {
                    st.newer = p.clone();
                    st.relation = None;
                }
                ui.label(egui::RichText::new(p).monospace().small());
            });
        }
    }

    let pair_ready = !st.older.is_empty() && !st.newer.is_empty() && st.older != st.newer;
    if pair_ready {
        ui.separator();
        // Work out how the pair ACTUALLY relate before offering an action on
        // them. Cheap enough to do inline: relate() clones into a temp dir and
        // counts, and it only runs when the pair changes.
        ui.horizontal(|ui| {
            if ui.small_button("check relation").clicked() {
                let r = cockpit::identity::relate(
                    std::path::Path::new(&st.older),
                    std::path::Path::new(&st.newer),
                    &std::env::var("PATH").unwrap_or_default());
                st.relation = Some((r.relation, r.a_ahead, r.b_ahead));
            }
            if let Some((rel, a, b)) = st.relation.clone() {
                let hint = relation_hint(&rel, a, b);
                let col = if can_fast_forward(&rel) { ui.visuals().text_color() }
                          else { egui::Color32::from_rgb(230, 170, 80) };
                ui.colored_label(col, hint);
            }
        });
        ui.horizontal(|ui| {
            if ui.button("preview roll-up").clicked() {
                let (o, n) = (st.older.clone(), st.newer.clone());
                start(&app.task, st, Pending::Rollup, move || {
                    enc_rollup(&cockpit::flow::rollup_preview(std::path::Path::new(&o), std::path::Path::new(&n)))
                });
            }
            // Gated on the relation when we know it. Offering a fast-forward
            // that cannot succeed produces a git error the page reports
            // verbatim, which is a poor way to find out the pair was backwards.
            let ff_ok = st.relation.as_ref().map(|(r, _, _)| can_fast_forward(r)).unwrap_or(true);
            if ui.add_enabled(ff_ok, egui::Button::new("apply roll-up (ff-only)")).clicked() {
                let (o, n) = (st.older.clone(), st.newer.clone());
                start(&app.task, st, Pending::RollupApply, move || {
                    match cockpit::flow::rollup_apply(std::path::Path::new(&o), std::path::Path::new(&n)) {
                        Ok(m) => format!("{OK}{m}"),
                        Err(e) => format!("{ERR}{e}"),
                    }
                });
            }
            if ui.button("explore merge \u{2192} newer").clicked() {
                let (o, n) = (st.older.clone(), st.newer.clone());
                start(&app.task, st, Pending::Explore, move || match cockpit::flow::explore(&o, &n) {
                    Ok(e) => enc_expl(&e),
                    Err(e) => format!("{ERR}{e}"),
                });
            }
        });
    }

    if let Some(r) = st.rollup.clone() {
        ui.separator();
        ui.label(format!(
            "relation: {} \u{2022} behind {} / ahead {} \u{2022} fast-forward: {}",
            r.relation,
            r.behind,
            r.ahead,
            if r.can_ff { "yes" } else { "no" }
        ));
        for (oid, subj) in r.incoming.iter().take(12) {
            ui.label(egui::RichText::new(format!("{oid} {subj}")).monospace().small());
        }
        for (stat, path) in r.files.iter().take(12) {
            ui.label(egui::RichText::new(format!("{stat} {path}")).monospace().weak().small());
        }
    }

    if let Some(e) = st.expl.clone() {
        ui.separator();
        ui.label(
            egui::RichText::new(format!("exploration into {} (target branch {})", e.workspace, e.target_branch)).small(),
        );
        for t in &e.trials {
            ui.horizontal(|ui| {
                ui.label(if t.ok { "\u{2713}" } else { "\u{2717}" });
                ui.label(egui::RichText::new(&t.label).strong());
                ui.label(
                    egui::RichText::new(format!("{} commit(s), {} conflict(s)", t.commits, t.conflicts)).weak().small(),
                );
                if t.ok && ui.small_button("preview").clicked() {
                    let (ws, tb, br) = (e.workspace.clone(), e.target_branch.clone(), t.branch.clone());
                    // stash the trial identity for the decode side
                    st.note = format!("{}\x1f{}", t.id, t.branch);
                    start(&app.task, st, Pending::Preview, move || {
                        match cockpit::flow::merge_preview(&ws, &tb, &br) {
                            Ok(p) => enc_preview(&p),
                            Err(e) => format!("{ERR}{e}"),
                        }
                    });
                }
            });
            if !t.summary.is_empty() {
                ui.label(egui::RichText::new(&t.summary).weak().small());
            }
        }
    }

    if let Some(p) = st.preview.clone() {
        ui.separator();
        ui.label(egui::RichText::new(format!("strategy {} \u{2014} {}", p.trial_id, p.shortstat)).strong());
        for (oid, subj) in p.commits.iter().take(12) {
            ui.label(egui::RichText::new(format!("{oid} {subj}")).monospace().small());
        }
        for (stat, path) in p.files.iter().take(16) {
            ui.label(egui::RichText::new(format!("{stat} {path}")).monospace().weak().small());
        }
        if ui.button(format!("land {} as a new branch", p.branch)).clicked() {
            if let Some(e) = st.expl.clone() {
                let (ws, br, target) = (e.workspace.clone(), p.branch.clone(), st.newer.clone());
                start(&app.task, st, Pending::Land, move || {
                    match cockpit::flow::land_merge(&ws, &br, &target, "") {
                        Ok(m) => format!("{OK}{m}"),
                        Err(e) => format!("{ERR}{e}"),
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_round_trip_the_wire() {
        let gs = vec![
            ("proj".to_string(), vec!["/a/proj".to_string(), "/b/proj".to_string()]),
            ("other".to_string(), vec!["/x".to_string(), "/y".to_string(), "/z".to_string()]),
        ];
        let enc = enc_groups(&gs);
        let body = ok_body(&enc).unwrap();
        let dec = dec_groups(body);
        assert_eq!(dec.len(), 2);
        assert_eq!(dec[0].name, "proj");
        assert_eq!(dec[0].paths, vec!["/a/proj", "/b/proj"]);
        assert_eq!(dec[1].paths.len(), 3);
    }

    #[test]
    fn rollup_round_trips_including_empty_lists() {
        let r = cockpit::flow::RollupPreview {
            relation: "behind".into(),
            can_fast_forward: true,
            behind: 4,
            ahead: 0,
            incoming: vec![("ab12".into(), "fix: things".into())],
            files: vec![],
        };
        let v = dec_rollup(ok_body(&enc_rollup(&r)).unwrap()).unwrap();
        assert_eq!(v.relation, "behind");
        assert!(v.can_ff);
        assert_eq!((v.behind, v.ahead), (4, 0));
        assert_eq!(v.incoming, vec![("ab12".to_string(), "fix: things".to_string())]);
        assert!(v.files.is_empty());
    }

    #[test]
    fn exploration_round_trips_and_multiline_summaries_flatten() {
        let e = cockpit::merge::Exploration {
            workspace: "/tmp/gm-merge-1".into(),
            target_branch: "main".into(),
            related: true,
            trials: vec![cockpit::merge::Trial {
                id: "m1".into(),
                label: "merge (ours)".into(),
                ok: true,
                conflicts: 0,
                commits: 7,
                summary: "line1\nline2".into(),
                recommended: false,
                branch: "gm-m1".into(),
            }],
        };
        let v = dec_expl(ok_body(&enc_expl(&e)).unwrap()).unwrap();
        assert_eq!(v.workspace, "/tmp/gm-merge-1");
        assert_eq!(v.target_branch, "main");
        assert_eq!(v.trials.len(), 1);
        let t = &v.trials[0];
        assert_eq!((t.ok, t.conflicts, t.commits), (true, 0, 7));
        assert_eq!(t.branch, "gm-m1");
        assert!(!t.summary.contains('\n')); // flattened, wire stays line-oriented
    }

    #[test]
    fn preview_round_trips_and_errors_pass_through() {
        let p = cockpit::flow::MergePreview {
            shortstat: " 3 files changed".into(),
            commits: vec![("aa".into(), "s1".into()), ("bb".into(), "s2".into())],
            files: vec![("M".into(), "src/lib.rs".into())],
        };
        let v = dec_preview(ok_body(&enc_preview(&p)).unwrap()).unwrap();
        assert_eq!(v.shortstat.trim(), "3 files changed");
        assert_eq!(v.commits.len(), 2);
        assert_eq!(v.files[0], ("M".to_string(), "src/lib.rs".to_string()));
        assert_eq!(ok_body("err\x1eboom"), Err("boom".to_string()));
        assert!(ok_body("garbage").is_err());
    }
}

#[cfg(test)]
mod relation_tests {
    use super::*;

    #[test]
    fn a_fast_forward_is_only_offered_when_it_can_succeed() {
        // The page let the user LABEL one copy older and one newer with nothing
        // checking it, then offered --ff-only. Getting it backwards produced a
        // raw git error, which is a poor way to learn you picked the wrong side.
        assert!(can_fast_forward("behind"), "the older copy really is behind");
        assert!(can_fast_forward("same"), "nothing to do, but it will not fail");
        assert!(!can_fast_forward("diverged"), "each has commits the other lacks");
        assert!(!can_fast_forward("unrelated"));
        assert!(!can_fast_forward("ahead"), "the pair is the wrong way round");
    }

    #[test]
    fn the_wrong_way_round_says_so_rather_than_saying_ahead() {
        // "ahead" is git's word for a fact about the pair; the USER needs to
        // know they mislabelled the copies, which is a different sentence.
        let h = relation_hint("ahead", 12, 0);
        assert!(h.contains("wrong way round"), "{h}");
        assert!(h.contains("12"), "and how far: {h}");
    }

    #[test]
    fn diverged_points_at_the_action_that_can_work() {
        // A dead end is only useful if it names the way out — the page has a
        // merge exploration for exactly this case.
        let h = relation_hint("diverged", 3, 5);
        assert!(h.contains("merge"), "{h}");
        assert!(h.contains('3') && h.contains('5'), "counts both ways: {h}");
    }

    #[test]
    fn identical_copies_say_there_is_nothing_to_do() {
        // Rather than offering a roll-up that would succeed and change nothing,
        // which reads as the tool having done something.
        let h = relation_hint("same", 0, 0);
        assert!(h.contains("nothing"), "{h}");
    }

    #[test]
    fn unrelated_histories_are_not_two_copies_of_one_project() {
        // The grouping is by project key, which can collide — two repos with no
        // git history fall back to a path hash. Saying "unrelated" is more
        // useful than offering to merge them.
        let h = relation_hint("unrelated", 0, 0);
        assert!(h.contains("not two copies"), "{h}");
    }
}
