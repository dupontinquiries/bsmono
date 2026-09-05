//! two_pane — a controls column and the thing it controls.
//!
//! Two apps in this tree have the same shape and the same fault: dandere2x
//! puts its settings in a 340pt left panel beside a preview, beatspace puts
//! its morph controls in a 250pt left panel beside the latent pad. Both are
//! reasonable on a laptop. On a 400pt phone the panel is most of the screen
//! and the content it controls is a sliver.
//!
//! The obvious fix — make the panel a fraction of the width — is worse, and
//! I shipped it to beatspace before looking at it: at 42% the pad became
//! usable and the panel's own controls started clipping. A narrow screen
//! does not want a narrower split, it wants a DIFFERENT ARRANGEMENT.
//!
//! So: side by side when both panes can have their minimum, stacked
//! otherwise — with the controls collapsed by default, because on a phone
//! the content is what you came for and the controls are a thing you open,
//! change and close.
//!
//! The decision is pure and tested; [`two_pane_ui`] applies it.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TwoPane {
    /// Both fit. `controls_w` is the controls column's width.
    SideBySide { controls_w: f32 },
    /// Not enough width for both. Controls go above the content, behind a
    /// disclosure so they cost one row when shut.
    Stacked,
}

impl TwoPane {
    pub fn is_stacked(self) -> bool { matches!(self, TwoPane::Stacked) }
}

/// Decide the arrangement.
///
/// `want_controls` is the controls column's preferred width; it is given
/// that much only while the content keeps `min_content`. Between the two
/// minima the controls shrink rather than the content, because a slider
/// with less room is still a slider and a preview with less room stops
/// being a preview.
pub fn plan(avail_w: f32, want_controls: f32, min_controls: f32, min_content: f32) -> TwoPane {
    let avail = avail_w.max(0.0);
    if avail < min_controls + min_content {
        return TwoPane::Stacked;
    }
    let controls_w = want_controls.min(avail - min_content).max(min_controls);
    TwoPane::SideBySide { controls_w }
}

/// Lay the two panes out. `id` keys the disclosure state so a page that
/// stacks remembers whether its controls were open.
///
/// Returns the arrangement used, so the caller can adapt its own content
/// (a preview might draw smaller when stacked).
/// `state` is threaded through both closures rather than captured by them:
/// a caller whose panes both need `&mut self` (which is every real one)
/// cannot build two closures that each borrow it, even though they run one
/// after the other. Passing it in is what makes the borrow checker agree
/// with the order the code actually executes in.
pub fn two_pane_ui<T>(
    ui: &mut egui::Ui,
    id: &str,
    want_controls: f32,
    min_controls: f32,
    min_content: f32,
    controls_label: &str,
    state: &mut T,
    controls: impl FnOnce(&mut egui::Ui, &mut T),
    content: impl FnOnce(&mut egui::Ui, &mut T),
) -> TwoPane {
    let arrangement = plan(ui.available_width(), want_controls, min_controls, min_content);
    match arrangement {
        TwoPane::SideBySide { controls_w } => {
            egui::Panel::left(egui::Id::new(("two_pane", id)))
                .resizable(true)
                .default_size(controls_w)
                .show_inside(ui, |ui| controls(ui, state));
            egui::CentralPanel::default().show_inside(ui, |ui| content(ui, state));
        }
        TwoPane::Stacked => {
            // Shut by default: the content is why the screen is on. Opening
            // is one tap and the state is remembered per page.
            egui::CollapsingHeader::new(controls_label)
                .id_salt(("two_pane_open", id))
                .default_open(false)
                .show(ui, |ui| controls(ui, state));
            ui.separator();
            content(ui, state);
        }
    }
    arrangement
}

#[cfg(test)]
mod tests {
    use super::*;

    // dandere2x's numbers: a 340pt settings column beside a preview.
    const WANT: f32 = 340.0;
    const MIN_C: f32 = 260.0;
    const MIN_V: f32 = 320.0;

    #[test]
    fn a_laptop_gets_both_panes_at_the_preferred_width() {
        assert_eq!(plan(1200.0, WANT, MIN_C, MIN_V), TwoPane::SideBySide { controls_w: WANT });
        assert_eq!(plan(660.0, WANT, MIN_C, MIN_V), TwoPane::SideBySide { controls_w: 340.0 });
    }

    #[test]
    fn a_phone_stacks_rather_than_splitting_into_two_useless_slivers() {
        // 400pt cannot give 260 to controls and 320 to content.
        assert_eq!(plan(400.0, WANT, MIN_C, MIN_V), TwoPane::Stacked);
        assert!(plan(400.0, WANT, MIN_C, MIN_V).is_stacked());
        // The boundary is exactly the sum of the minima.
        assert_eq!(plan(579.0, WANT, MIN_C, MIN_V), TwoPane::Stacked);
        assert_eq!(plan(580.0, WANT, MIN_C, MIN_V), TwoPane::SideBySide { controls_w: 260.0 });
    }

    #[test]
    fn between_the_minima_the_controls_give_way_and_never_below_their_own_floor() {
        // A slider with less room is still a slider; a preview with less room
        // stops being a preview — so the content's minimum is the one held.
        let p = plan(600.0, WANT, MIN_C, MIN_V);
        match p {
            TwoPane::SideBySide { controls_w } => {
                assert_eq!(controls_w, 280.0, "600 - 320 of content");
                assert!(controls_w >= MIN_C);
                assert!(600.0 - controls_w >= MIN_V);
            }
            other => panic!("{other:?}"),
        }
        // Never below the controls' own floor, whatever the arithmetic says.
        match plan(580.0, WANT, MIN_C, MIN_V) {
            TwoPane::SideBySide { controls_w } => assert_eq!(controls_w, MIN_C),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_zero_or_absurd_width_is_stacked_rather_than_a_negative_column() {
        assert!(plan(0.0, WANT, MIN_C, MIN_V).is_stacked());
        assert!(plan(-50.0, WANT, MIN_C, MIN_V).is_stacked());
        // And beatspace's smaller numbers still behave.
        assert!(plan(400.0, 250.0, 200.0, 240.0).is_stacked());
        assert_eq!(plan(900.0, 250.0, 200.0, 240.0), TwoPane::SideBySide { controls_w: 250.0 });
    }
}
