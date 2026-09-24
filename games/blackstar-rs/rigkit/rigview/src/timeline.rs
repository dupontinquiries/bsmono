//! Each panel's dissolve progress at a timeline value, as the page's refresh
//! works it out: streamed parts get at least 0.36 of window, and a streamed
//! part is hidden once its progress reaches 0.999.

pub fn progress(t: f32, dis: [f32; 2], stream: bool) -> f32 {
    let dur = if stream { dis[1].max(0.36) } else { dis[1] };
    ((t - dis[0]) / dur).clamp(0.0, 1.0)
}

pub fn visible(u: f32, stream: bool) -> bool {
    !(stream && u >= 0.999)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streamed_windows_are_at_least_036() {
        assert!((progress(0.3 + 0.18, [0.3, 0.1], true) - 0.5).abs() < 1e-6);
    }
    #[test]
    fn a_finished_streamed_part_is_hidden() {
        assert!(!visible(1.0, true));
        assert!(visible(1.0, false));
    }
}
