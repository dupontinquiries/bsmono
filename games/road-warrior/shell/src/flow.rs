//! The shell's screens: title (with the car select the unlocks never had a way to reach), play,
//! pause, and the campaign-complete screen that was a toast.
//!
//! The shell started in the game with a car chosen by a heuristic ("Pillager if unlocked, else
//! Behemoth, ..."), had no pause, and said "Campaign complete!" in a three-second toast while
//! the last level sat there finished. Pure: keys in, actions out, so `main.rs` only draws and
//! acts. `simulates()` is the one question the loop asks every frame.

use road_warrior_core::CarKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// `car` indexes the UNLOCKED cars (`SaveState::unlocked_cars`), wrapping.
    Title { car: usize },
    Playing,
    Paused,
    CampaignComplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Enter / Space on a screen (Space is the launch key in play, so play ignores it here).
    Confirm,
    /// Escape.
    Back,
    Left,
    Right,
}

/// What `main.rs` does in response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Start (or restart after the campaign) driving `CarKind` from the saved level.
    Start(CarKind),
    Resume,
    /// Back to the title from the pause screen: the level is abandoned.
    QuitToTitle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flow {
    pub screen: Screen,
}

impl Flow {
    /// Opens on the title, with the most recently unlocked car selected (the last in the
    /// unlocked list -- what the old heuristic preferred).
    pub fn new(unlocked: &[CarKind]) -> Flow {
        Flow { screen: Screen::Title { car: unlocked.len().saturating_sub(1) } }
    }

    /// Does the simulation step this frame?
    pub fn simulates(&self) -> bool {
        self.screen == Screen::Playing
    }

    /// The campaign's last level was completed: the screen for it.
    pub fn campaign_finished(&mut self) {
        self.screen = Screen::CampaignComplete;
    }

    /// A key press on the current screen. `unlocked` is the car list the title chooses from;
    /// an empty list (a corrupt save) still starts, with the starting car.
    pub fn on_key(&mut self, key: Key, unlocked: &[CarKind]) -> Option<Action> {
        match (self.screen, key) {
            (Screen::Title { car }, Key::Left) => {
                let n = unlocked.len().max(1);
                self.screen = Screen::Title { car: (car + n - 1) % n };
                None
            }
            (Screen::Title { car }, Key::Right) => {
                let n = unlocked.len().max(1);
                self.screen = Screen::Title { car: (car + 1) % n };
                None
            }
            (Screen::Title { car }, Key::Confirm) => {
                let kind = unlocked.get(car).copied().unwrap_or(CarKind::RedSedan);
                self.screen = Screen::Playing;
                Some(Action::Start(kind))
            }
            (Screen::Playing, Key::Back) => {
                self.screen = Screen::Paused;
                None
            }
            (Screen::Paused, Key::Back) | (Screen::Paused, Key::Confirm) => {
                self.screen = Screen::Playing;
                Some(Action::Resume)
            }
            (Screen::Paused, Key::Left) | (Screen::Paused, Key::Right) => {
                self.screen = Screen::Title { car: 0 };
                Some(Action::QuitToTitle)
            }
            (Screen::CampaignComplete, Key::Confirm) | (Screen::CampaignComplete, Key::Back) => {
                self.screen = Screen::Title { car: unlocked.len().saturating_sub(1) };
                None
            }
            _ => None,
        }
    }

    /// The car the title has selected, for drawing.
    pub fn selected_car(&self, unlocked: &[CarKind]) -> Option<CarKind> {
        match self.screen {
            Screen::Title { car } => unlocked.get(car).copied().or(Some(CarKind::RedSedan)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: [CarKind; 2] = [CarKind::RedSedan, CarKind::SixWheeler];

    #[test]
    fn the_title_selects_among_unlocked_cars_and_starts_the_chosen_one() {
        let mut f = Flow::new(&TWO);
        assert_eq!(f.screen, Screen::Title { car: 1 }, "the newest unlock is selected first");
        assert!(!f.simulates(), "nothing drives on the title");
        assert_eq!(f.on_key(Key::Right, &TWO), None);
        assert_eq!(f.selected_car(&TWO), Some(CarKind::RedSedan), "wraps");
        assert_eq!(f.on_key(Key::Left, &TWO), None);
        assert_eq!(f.selected_car(&TWO), Some(CarKind::SixWheeler));
        assert_eq!(f.on_key(Key::Confirm, &TWO), Some(Action::Start(CarKind::SixWheeler)));
        assert!(f.simulates());
        assert_eq!(f.selected_car(&TWO), None);
    }

    #[test]
    fn a_locked_car_cannot_be_chosen_and_an_empty_list_still_starts() {
        let one = [CarKind::RedSedan];
        let mut f = Flow::new(&one);
        for _ in 0..5 {
            f.on_key(Key::Right, &one);
            assert_eq!(f.selected_car(&one), Some(CarKind::RedSedan));
        }
        let mut f = Flow::new(&[]);
        assert_eq!(f.on_key(Key::Confirm, &[]), Some(Action::Start(CarKind::RedSedan)));
    }

    #[test]
    fn pause_stops_the_sim_resumes_on_esc_or_enter_and_quits_sideways() {
        let mut f = Flow::new(&TWO);
        f.on_key(Key::Confirm, &TWO);
        assert_eq!(f.on_key(Key::Confirm, &TWO), None, "Confirm in play is the launch key, not a flow key");
        assert!(f.simulates());
        assert_eq!(f.on_key(Key::Back, &TWO), None);
        assert_eq!(f.screen, Screen::Paused);
        assert!(!f.simulates());
        assert_eq!(f.on_key(Key::Back, &TWO), Some(Action::Resume));
        assert!(f.simulates());
        f.on_key(Key::Back, &TWO);
        assert_eq!(f.on_key(Key::Confirm, &TWO), Some(Action::Resume));
        f.on_key(Key::Back, &TWO);
        assert_eq!(f.on_key(Key::Left, &TWO), Some(Action::QuitToTitle));
        assert!(matches!(f.screen, Screen::Title { .. }));
    }

    #[test]
    fn the_campaign_complete_screen_holds_until_a_key_and_returns_to_the_title() {
        let mut f = Flow::new(&TWO);
        f.on_key(Key::Confirm, &TWO);
        f.campaign_finished();
        assert!(!f.simulates(), "the finished level does not keep driving under the screen");
        assert_eq!(f.on_key(Key::Left, &TWO), None, "only Confirm/Back leave it");
        assert_eq!(f.screen, Screen::CampaignComplete);
        assert_eq!(f.on_key(Key::Confirm, &TWO), None);
        assert_eq!(f.screen, Screen::Title { car: 1 });
    }
}
