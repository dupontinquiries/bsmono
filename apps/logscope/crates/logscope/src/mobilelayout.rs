//! mobilelayout.rs — what changes on a phone, and what must not.
//!
//! The temptation is to shrink the desktop layout. That fails for a reason worth naming: **a phone
//! is not a small screen, it is a different set of constraints**, and only some of them are about
//! size.
//!
//! * **Touch targets have a floor.** 44pt is not a style preference; below it a control is
//!   unreliable for people whose hands shake, and shrinking to fit produces a UI that works in a
//!   screenshot and not in a hand.
//! * **The keyboard eats half the screen.** Any layout measured against the full height is wrong
//!   whenever someone is typing, which is most of when they are using it.
//! * **Battery makes background work a cost the desktop does not pay.** A fan seat that is free on a
//!   plugged-in workstation drains a phone the operator needs later.
//! * **The network is metered and intermittent.** Streaming weights over cellular is not slow, it is
//!   expensive to someone.
//!
//! Size is the least interesting of the four and the only one shrinking addresses.

/// What the app knows about the device it is on.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Viewport {
    pub width_dp: f32,
    pub height_dp: f32,
    /// Height taken by the software keyboard right now, zero when hidden.
    pub keyboard_dp: f32,
    pub on_battery: bool,
    pub battery_frac: f32,
    /// Cellular or a metered hotspot.
    pub metered: bool,
}

/// The minimum comfortable touch target.
///
/// **A floor, not a preference.** Below this a control is unreliable for anyone whose hands are not
/// steady, and a layout that shrinks controls to fit works in a screenshot and not in a hand.
pub const MIN_TOUCH_DP: f32 = 44.0;

impl Viewport {
    pub fn is_phone(&self) -> bool { self.width_dp < 600.0 }

    /// Height actually available for content.
    ///
    /// **Not the viewport height.** A layout measured against the full height is wrong whenever
    /// someone is typing, which is most of when they are using it.
    pub fn usable_height_dp(&self) -> f32 { (self.height_dp - self.keyboard_dp).max(0.0) }

    /// How many rows of a list fit, given a row height.
    ///
    /// Returns zero rather than a negative when the keyboard covers everything, because a caller
    /// that trusts a signed count will index off the end of a Vec.
    pub fn rows_that_fit(&self, row_dp: f32) -> usize {
        if row_dp <= 0.0 { return 0 }
        (self.usable_height_dp() / row_dp).floor().max(0.0) as usize
    }

    /// May this device host a fan seat?
    pub fn can_host_background_work(&self) -> Result<(), String> {
        if self.on_battery && self.battery_frac < 0.4 {
            return Err(format!("{:.0}% battery — a fan seat is free on a plugged-in workstation and \
drains a phone the operator needs later", self.battery_frac * 100.0));
        }
        if self.metered {
            return Err("metered connection — streaming weights over cellular is not slow, it is \
expensive to someone, and the cost lands on them rather than on the run".into());
        }
        Ok(())
    }
}

/// A control's size, after the floor is applied.
///
/// Returns what was requested and what it became, so a layout can tell whether it had to grow
/// something — and therefore whether the row it planned still fits.
pub fn touch_size(requested_dp: f32) -> (f32, bool) {
    if requested_dp < MIN_TOUCH_DP { (MIN_TOUCH_DP, true) } else { (requested_dp, false) }
}

/// How many controls fit across, at the touch floor.
///
/// **Fewer than a naive division suggests**, because a control grown to the floor takes the space
/// the layout thought it had spare. A toolbar designed for six buttons on desktop becomes four, and
/// discovering that at runtime rather than at design time is how a phone UI ends up with a row that
/// overflows.
pub fn controls_across(width_dp: f32, requested_dp: f32, gap_dp: f32) -> usize {
    let (size, _) = touch_size(requested_dp);
    if size + gap_dp <= 0.0 { return 0 }
    (((width_dp + gap_dp) / (size + gap_dp)).floor()).max(0.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(kb: f32) -> Viewport {
        Viewport { width_dp: 390.0, height_dp: 844.0, keyboard_dp: kb,
                   on_battery: true, battery_frac: 0.9, metered: false }
    }

    #[test]
    fn the_KEYBOARD_is_subtracted_from_usable_height() {
        // A layout measured against the full height is wrong whenever someone is typing, which is
        // most of when they are using it.
        assert_eq!(phone(0.0).usable_height_dp(), 844.0);
        assert_eq!(phone(400.0).usable_height_dp(), 444.0);
    }

    #[test]
    fn rows_never_go_NEGATIVE_when_the_keyboard_covers_everything() {
        // A caller trusting a signed count would index off the end of a Vec.
        assert_eq!(phone(900.0).rows_that_fit(60.0), 0);
    }

    #[test]
    fn a_TOUCH_TARGET_is_grown_to_the_floor_and_says_it_grew() {
        // The caller needs to know, because a grown control takes space the layout thought it had.
        assert_eq!(touch_size(24.0), (MIN_TOUCH_DP, true));
        assert_eq!(touch_size(60.0), (60.0, false));
    }

    #[test]
    fn FEWER_controls_fit_than_a_naive_division_suggests() {
        // A toolbar designed for six 30dp buttons becomes four at the 44dp floor, and finding that
        // at runtime is how a phone row overflows.
        let naive = ((390.0f32 + 8.0) / (30.0f32 + 8.0)).floor() as usize;
        let real = controls_across(390.0, 30.0, 8.0);
        assert!(real < naive, "naive {naive}, real {real}");
    }

    #[test]
    fn a_LOW_BATTERY_phone_declines_background_work() {
        // Free on a plugged-in workstation, and a cost to someone who needs the phone later.
        let mut v = phone(0.0);
        v.battery_frac = 0.2;
        assert!(v.can_host_background_work().unwrap_err().contains("needs later"));
    }

    #[test]
    fn a_METERED_connection_declines_regardless_of_battery() {
        // The cost lands on the operator rather than on the run, which is why it is not a speed
        // tradeoff to weigh.
        let mut v = phone(0.0);
        v.metered = true;
        assert!(v.can_host_background_work().unwrap_err().contains("expensive to someone"));
    }

    #[test]
    fn a_CHARGING_phone_on_wifi_is_allowed() {
        // The rules must permit the good case or the phone is decorative.
        let mut v = phone(0.0);
        v.on_battery = false;
        assert!(v.can_host_background_work().is_ok());
    }

    #[test]
    fn a_TABLET_is_not_treated_as_a_phone() {
        let tablet = Viewport { width_dp: 1024.0, height_dp: 768.0, keyboard_dp: 0.0,
                                on_battery: true, battery_frac: 0.9, metered: false };
        assert!(!tablet.is_phone());
    }
}
