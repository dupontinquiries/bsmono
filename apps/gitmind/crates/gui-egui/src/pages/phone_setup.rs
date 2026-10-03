//! Phone setup -- replay the Fairphone 5 customisations from the GUI.
//!
//! `cockpit::fp5_setup` holds every hand-applied change as a plan (a list of steps, each with a
//! gate that must pass and a rollback). Until now nothing called it: `gitmind fp5 apply` was the
//! only way in, and the GUI had no page. This is the section that was missing, on the Network
//! page beside the other per-device controls, using the same target (`app.net.user/host`) and the
//! same background task, so a plan cannot run while a deploy does.
//!
//! * Each part can be inspected first: **plan** shows every command and its gate, and what undoes it.
//! * **apply** runs one part; **apply all** runs them in order and stops at the first failure
//!   (the plan runner has already rolled that part back).
//! * Root steps need the account's password. It is held in a masked field, goes to ssh's STDIN
//!   (never an argument, never the log), and is cleared after a run.

use crate::app::App;
use cockpit::cosmic_setup::{run_over_ssh, Root};
use cockpit::patchset::Step;

#[derive(Default)]
pub struct State {
    /// The account's password, for steps that need root. Masked in the UI, cleared after a run.
    pub password: String,
    /// The part whose plan is expanded.
    pub open: Option<usize>,
    /// How the last run ended: "ok" or "failed: ...".
    pub last: String,
}

/// Root works through `sudo -S` when there is a password, and is attempted bare otherwise.
pub fn root_for(password: &str) -> Root {
    if password.is_empty() {
        Root::NoPrompt
    } else {
        Root::PasswordOnStdin
    }
}

/// The parts, in replay order, as (name, steps).
pub fn parts(password: &str) -> Vec<(&'static str, Vec<Step>)> {
    cockpit::fp5_setup::all_parts(root_for(password))
}

/// Parts that change how the phone can be entered (autologin): offered, planned and applied one at a
/// time, and never part of "apply all".
pub fn opt_in(password: &str) -> Vec<(&'static str, Vec<Step>)> {
    cockpit::fp5_setup::opt_in_parts(root_for(password))
}

/// A part's plan as plain text: what each step is for, the command, and what undoes it.
pub fn plan_text(steps: &[Step]) -> String {
    let mut out = String::new();
    for (i, s) in steps.iter().enumerate() {
        out.push_str(&format!("{}. {}\n   $ {}\n", i + 1, s.what, s.cmd));
        if let Some(r) = &s.rollback {
            out.push_str(&format!("   undo: {r}\n"));
        }
    }
    out
}

/// A run needs a target, and the shared task slot free.
pub fn can_run(user: &str, host: &str, task_running: bool) -> bool {
    !user.trim().is_empty() && !host.trim().is_empty() && !task_running
}

/// Run `steps` on the target, logging each step; returns "ok" or "failed: ...".
fn run(steps: Vec<Step>, user: String, host: String, password: String, sink: &crate::task::LogSink) -> String {
    let pw = (!password.is_empty()).then_some(password.as_str());
    match run_over_ssh(&steps, &user, &host, pw, &mut |l| sink.push(l)) {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("failed: {e}"),
    }
}

pub fn section(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Phone setup");
    ui.label("Replay the Fairphone 5 changes (keyboard service, panel zones, watchdog, ...) on the target above.");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("password (for root steps)");
        ui.add(egui::TextEdit::singleline(&mut app.phone_setup.password).password(true).id_salt("phone_setup-password"));
    });
    let (user, host) = (app.net.user.clone(), app.net.host.clone());
    let can = can_run(&user, &host, app.task.is_running());
    if !can && !user.trim().is_empty() && !host.trim().is_empty() {
        ui.weak("waiting for the running task to finish");
    } else if user.trim().is_empty() || host.trim().is_empty() {
        ui.weak("set a user and host above first");
    }
    let mut all = parts(&app.phone_setup.password);
    let n_normal = all.len();
    all.extend(opt_in(&app.phone_setup.password));
    ui.add_space(4.0);
    let mut run_one: Option<usize> = None;
    for (i, (name, steps)) in all.iter().enumerate() {
        ui.horizontal(|ui| {
            let tag = if i >= n_normal { "  [opt-in: only by itself]" } else { "" };
            ui.label(format!("{name}  ({} steps){tag}", steps.len()));
            if ui.small_button(if app.phone_setup.open == Some(i) { "hide plan" } else { "plan" }).clicked() {
                app.phone_setup.open = if app.phone_setup.open == Some(i) { None } else { Some(i) };
            }
            if ui.add_enabled(can, egui::Button::new("apply").small()).clicked() {
                run_one = Some(i);
            }
        });
        if app.phone_setup.open == Some(i) {
            ui.label(egui::RichText::new(plan_text(steps)).monospace().small());
        }
    }
    ui.add_space(4.0);
    let mut run_all = false;
    if ui.add_enabled(can, egui::Button::new("apply all")).clicked() {
        run_all = true;
    }
    if run_one.is_some() || run_all {
        let steps: Vec<(String, Vec<Step>)> = match run_one {
            Some(i) => vec![(all[i].0.to_string(), all[i].1.clone())],
            None => all[..n_normal].iter().map(|(n, s)| (n.to_string(), s.clone())).collect(),
        };
        let password = std::mem::take(&mut app.phone_setup.password);
        app.phone_setup.last.clear();
        app.task.run_logged(move |sink| {
            for (name, s) in steps {
                sink.push(&format!("== {name}"));
                let r = run(s, user.clone(), host.clone(), password.clone(), &sink);
                if r != "ok" {
                    return format!("{name}: {r}");
                }
            }
            "ok".to_string()
        });
    }
    if !app.task.is_running() {
        if let Some(r) = app.task.take_result() {
            app.phone_setup.last = r;
        }
    }
    if !app.phone_setup.last.is_empty() {
        ui.label(format!("last run: {}", app.phone_setup.last));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parts_are_the_fp5_plans_in_replay_order_with_steps_to_show() {
        let p = parts("");
        assert!(p.len() >= 8, "the scan found the parts: {}", p.len());
        assert!(p.iter().all(|(_, s)| !s.is_empty()));
        let names: Vec<&str> = p.iter().map(|(n, _)| *n).collect();
        for want in ["keyboard service", "panel memory watchdog", "cpu governor"] {
            assert!(names.contains(&want), "{want} is offered: {names:?}");
        }
    }

    #[test]
    fn opt_in_parts_are_separate_from_the_ordinary_ones() {
        let names: Vec<&str> = parts("").iter().map(|(n, _)| *n).collect();
        let opt: Vec<&str> = opt_in("").iter().map(|(n, _)| *n).collect();
        assert!(opt.iter().any(|n| n.contains("autologin")), "autologin is offered: {opt:?}");
        assert!(opt.iter().all(|n| !names.contains(n)), "and is not among the ordinary parts that apply-all runs");
    }

    #[test]
    fn a_password_selects_the_stdin_root_path_and_never_appears_in_a_plan() {
        let secret = "hunter2-not-real";
        assert_eq!(root_for(""), Root::NoPrompt);
        assert_eq!(root_for(secret), Root::PasswordOnStdin);
        let with = parts(secret);
        let text: String = with.iter().map(|(_, s)| plan_text(s)).collect();
        assert!(!text.contains(secret), "the password is not part of any command");
        assert!(text.contains("sudo -S"), "the root steps read it from stdin instead");
        let without: String = parts("").iter().map(|(_, s)| plan_text(s)).collect();
        assert!(!without.contains("sudo -S"), "no password, no sudo -S");
    }

    #[test]
    fn plan_text_lists_every_step_its_command_and_its_undo() {
        let p = parts("");
        let (_, steps) = p.iter().find(|(n, _)| *n == "panel memory watchdog").unwrap();
        let t = plan_text(steps);
        for (i, s) in steps.iter().enumerate() {
            assert!(t.contains(&format!("{}. {}", i + 1, s.what)), "step {i} named");
            assert!(t.contains(&s.cmd), "step {i} command shown");
        }
        assert!(t.contains("undo: "), "a step that can be undone says how");
    }

    #[test]
    fn a_run_needs_a_target_and_a_free_task_slot() {
        assert!(can_run("user", "192.168.2.231", false));
        assert!(!can_run("", "host", false), "no user");
        assert!(!can_run("user", "  ", false), "a blank host is no host");
        assert!(!can_run("user", "host", true), "not while another task runs: both would look stuck");
    }
}
