//! swarmview.rs — observability for the swarm.
//!
//! A PURE view-model (`telemetry`) turns the running swarm's state — blackboard findings, budget spend,
//! promoted tools — into renderable nodes and gauges, so the mapping is testable without egui. The gui-gated
//! `panel` renders it into an egui side panel; placing the same nodes as floating labels in the 3D palace
//! is the render-side follow-up (the view-model already carries everything it needs: kind, branch, text).

use logscope::blackboard::{Blackboard, Finding};
use logscope::budget::{Budget, Spend};
use logscope::toolreg::ToolRegistry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind { Fact, Deadend, Tool }
impl NodeKind {
    /// A stable glyph + color hint so the panel and the (future) 3D nodes agree on encoding.
    pub fn glyph(self) -> &'static str { match self { NodeKind::Fact => "✓", NodeKind::Deadend => "✗", NodeKind::Tool => "★" } }
    pub fn rgb(self) -> [u8; 3] { match self { NodeKind::Fact => [120, 200, 140], NodeKind::Deadend => [210, 120, 110], NodeKind::Tool => [220, 190, 120] } }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node { pub kind: NodeKind, pub branch: String, pub text: String }

#[derive(Clone, Debug, PartialEq)]
pub struct Gauge { pub label: String, pub fraction: f32, pub text: String }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Telemetry {
    pub headline: String,   // one-line status for a HUD strip
    pub nodes: Vec<Node>,   // findings, newest last (post order)
    pub gauges: Vec<Gauge>, // budget bars (attempts, tokens)
    pub tools: Vec<String>, // promoted-glue menu lines
    pub facts: usize,
    pub deadends: usize,
    pub tool_count: usize,
}

/// Build the telemetry snapshot from the live swarm components. Pure — no egui, fully unit-tested.
pub fn telemetry(board: &Blackboard, spend: &Spend, budget: &Budget, tools: &ToolRegistry) -> Telemetry {
    let mut nodes = Vec::with_capacity(board.entries().len());
    let (mut facts, mut deadends, mut tool_count) = (0usize, 0usize, 0usize);
    for e in board.entries() {
        let kind = match &e.finding {
            Finding::Fact(_) | Finding::Artifact { .. } => { facts += 1; NodeKind::Fact }
            Finding::Deadend(_) => { deadends += 1; NodeKind::Deadend }
            Finding::Tool(_) => { tool_count += 1; NodeKind::Tool }
        };
        nodes.push(Node { kind, branch: e.branch.clone(), text: e.finding.text().to_string() });
    }
    let frac = budget.fraction_used(spend);
    let gauges = vec![
        Gauge { label: "attempts".into(), fraction: (spend.attempts as f32 / budget.max_attempts.max(1) as f32).min(1.0), text: format!("{} / {}", spend.attempts, budget.max_attempts) },
        Gauge { label: "tokens".into(), fraction: (spend.tokens as f32 / budget.max_tokens.max(1) as f32).min(1.0), text: format!("{} / {}", spend.tokens, budget.max_tokens) },
    ];
    let tool_lines: Vec<String> = tools.menu().lines().filter(|l| !l.trim().is_empty()).map(|l| l.to_string()).collect();
    let headline = format!("{facts} facts · {deadends} deadends · {tool_count} tools · {:.0}% budget", frac * 100.0);
    Telemetry { headline, nodes, gauges, tools: tool_lines, facts, deadends, tool_count }
}

/// Render the telemetry into an egui panel (gui only). Deadends are dimmed, tools highlighted; the budget
/// bars turn amber past 80%. Read-only — the swarm owns the state; this only reflects it.
#[cfg(feature = "gui")]
pub fn panel(ui: &mut egui::Ui, tel: &Telemetry) {
    ui.label(egui::RichText::new(&tel.headline).strong());
    ui.separator();
    for g in &tel.gauges {
        let hot = g.fraction >= 0.8;
        ui.horizontal(|ui| {
            ui.label(format!("{:>8}", g.label));
            let bar = egui::ProgressBar::new(g.fraction).text(g.text.clone()).desired_width(160.0);
            ui.add(if hot { bar.fill(egui::Color32::from_rgb(200, 150, 60)) } else { bar });
        });
    }
    if !tel.tools.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("promoted glue").weak());
        for t in &tel.tools { ui.label(format!("★ {t}")); }
    }
    ui.separator();
    ui.label(egui::RichText::new("findings").weak());
    egui::ScrollArea::vertical().id_salt("swarm").max_height(220.0).show(ui, |ui| {
        for n in tel.nodes.iter().rev() {
            let [r, g, b] = n.kind.rgb();
            let col = egui::Color32::from_rgb(r, g, b);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(n.kind.glyph()).color(col));
                ui.label(egui::RichText::new(&n.branch).weak().small());
                ui.label(&n.text);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use logscope::blackboard::Finding;

    fn seeded() -> (Blackboard, Spend, Budget, ToolRegistry) {
        let mut b = Blackboard::new();
        b.post("subtask0", Finding::Fact("solved 0".into()), &["solved"]);
        b.post("subtask0", Finding::Deadend("type error".into()), &["deadend"]);
        b.post("subtask1", Finding::Tool("lib.rs#0".into()), &["tool"]);
        let mut sp = Spend::default();
        sp.add_attempt(1000, 50);
        sp.add_attempt(1000, 50);
        let bud = Budget::new(10_000, std::time::Duration::from_secs(60), 40);
        let mut tr = ToolRegistry::new();
        let _ = tr.promote(logscope::toolreg::Tool::new("lib.rs#0", "verified fill", "fn a()", logscope::lang::Lang::Rust), true);
        (b, sp, bud, tr)
    }

    #[test]
    fn telemetry_counts_and_maps_findings() {
        let (b, sp, bud, tr) = seeded();
        let t = telemetry(&b, &sp, &bud, &tr);
        assert_eq!((t.facts, t.deadends, t.tool_count), (1, 1, 1));
        assert_eq!(t.nodes.len(), 3, "one node per finding");
        assert!(t.nodes.iter().any(|n| n.kind == NodeKind::Deadend && n.text.contains("type error")));
        assert!(t.headline.contains("1 facts") && t.headline.contains("1 tools"));
    }

    #[test]
    fn telemetry_budget_gauges_reflect_spend() {
        let (b, sp, bud, tr) = seeded();
        let t = telemetry(&b, &sp, &bud, &tr);
        let attempts = t.gauges.iter().find(|g| g.label == "attempts").unwrap();
        assert!((attempts.fraction - 2.0 / 40.0).abs() < 1e-6, "2 of 40 attempts");
        let tokens = t.gauges.iter().find(|g| g.label == "tokens").unwrap();
        assert!((tokens.fraction - 2000.0 / 10_000.0).abs() < 1e-6, "2000 of 10000 tokens");
        assert!(!t.tools.is_empty(), "promoted tool surfaces in the menu");
    }

    #[test]
    fn empty_swarm_is_blank_not_broken() {
        let t = telemetry(&Blackboard::new(), &Spend::default(), &Budget::default_run(), &ToolRegistry::new());
        assert_eq!(t.nodes.len(), 0);
        assert_eq!((t.facts, t.deadends, t.tool_count), (0, 0, 0));
        assert_eq!(t.gauges.len(), 2, "gauges always present, at 0");
        assert!(t.gauges.iter().all(|g| g.fraction == 0.0));
    }
}
