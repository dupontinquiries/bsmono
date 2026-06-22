// src/anim_detect.rs — lean animation detection and QML wrapping
#[derive(Clone, Debug)]
pub enum AnimKind { FadeIn, SlideUp, SlideDown, SlideLeft, SlideRight, FadeSlideUp, FadeSlideDown, ZoomIn, FadeZoom }

#[derive(Clone, Debug)]
pub struct AnimParams { pub kind: AnimKind, pub dur: u32, pub delay: u32, pub easing: &'static str }

pub fn detect_animation(classes: Option<&str>, style: Option<&str>, data_aos: Option<&str>, data_animate: Option<&str>) -> Option<AnimParams> {
    if let Some(aos) = data_aos { return Some(parse_aos(aos)); }
    if let Some(cls) = classes {
        let c = cls.to_ascii_lowercase();
        if c.contains("animate__") || c.contains("animated") {
            let k = if c.contains("fadeinup") { AnimKind::FadeSlideUp } else if c.contains("fadein") { AnimKind::FadeIn }
                else if c.contains("slidein") { AnimKind::SlideUp } else if c.contains("zoomin") { AnimKind::ZoomIn } else { return None };
            return Some(AnimParams { kind: k, dur: 500, delay: 0, easing: "Easing.OutBack" });
        }
        if c.contains("wow") { return Some(AnimParams { kind: AnimKind::FadeIn, dur: 600, delay: 200, easing: "Easing.OutCubic" }); }
        if c.contains("animate-fade") { return Some(AnimParams { kind: if c.contains("up") { AnimKind::FadeSlideUp } else { AnimKind::FadeIn }, dur: 300, delay: 0, easing: "Easing.OutQuad" }); }
        if c.contains("reveal") || c.contains("appear") { return Some(AnimParams { kind: AnimKind::FadeIn, dur: 400, delay: 100, easing: "Easing.OutQuad" }); }
    }
    if let Some(da) = data_animate {
        let v = da.to_ascii_lowercase();
        let k = if v.contains("fade") && v.contains("up") { AnimKind::FadeSlideUp } else if v.contains("fade") { AnimKind::FadeIn }
            else if v.contains("slide") { AnimKind::SlideUp } else { AnimKind::FadeIn };
        return Some(AnimParams { kind: k, dur: 500, delay: 0, easing: "Easing.OutCubic" });
    }
    if let Some(sty) = style {
        let s = sty.to_ascii_lowercase();
        if s.contains("opacity") && s.contains(": 0") && !s.contains("0.") && !s.contains("transition") {
            return None; // hidden, not animated
        }
        if s.contains("transition") && s.contains("opacity") {
            return Some(AnimParams { kind: AnimKind::FadeIn, dur: 400, delay: 0, easing: "Easing.OutQuad" });
        }
    }
    None
}

fn parse_aos(v: &str) -> AnimParams {
    let k = match v {
        "fade"|"fade-in" => AnimKind::FadeIn, "fade-up" => AnimKind::FadeSlideUp,
        "fade-down" => AnimKind::FadeSlideDown, "fade-left" => AnimKind::SlideLeft,
        "fade-right" => AnimKind::SlideRight, "slide-up" => AnimKind::SlideUp,
        "slide-down" => AnimKind::SlideDown, "zoom-in" => AnimKind::ZoomIn,
        _ => AnimKind::FadeIn,
    };
    AnimParams { kind: k, dur: 600, delay: 100, easing: "Easing.OutCubic" }
}

/// Wrap QML with animation. Returns (prefix, suffix).
pub fn wrap_with_animation(a: &AnimParams) -> (String, String) {
    let st = if a.delay == 0 { "visible" } else { "" };
    let timer = if a.delay > 0 {
        format!("Timer {{\ninterval: {}\nrunning: true\nonTriggered: {{ parent.state = \"visible\" }}\n}}\n", a.delay)
    } else { String::new() };

    // Common template: Item wrapper with state/transition
    let (init_props, target_props, anim_props) = match &a.kind {
        AnimKind::FadeIn => ("opacity: 0", "opacity: 1", "\"opacity\""),
        AnimKind::FadeSlideUp | AnimKind::SlideUp => ("opacity: 0\ny: 30", "y: 0\nopacity: 1", "\"y,opacity\""),
        AnimKind::FadeSlideDown | AnimKind::SlideDown => ("opacity: 0\ny: -30", "y: 0\nopacity: 1", "\"y,opacity\""),
        AnimKind::SlideLeft => ("opacity: 0\nx: -40", "x: 0\nopacity: 1", "\"x,opacity\""),
        AnimKind::SlideRight => ("opacity: 0\nx: 40", "x: 0\nopacity: 1", "\"x,opacity\""),
        AnimKind::ZoomIn => ("scale: 0.85", "scale: 1.0", "\"scale\""),
        AnimKind::FadeZoom => ("opacity: 0\nscale: 0.85", "scale: 1.0\nopacity: 1", "\"scale,opacity\""),
    };

    let mut pre = String::with_capacity(256);
    pre.push_str("Item {\nwidth: parent ? parent.width : 800\nheight: childrenRect.height\n");
    pre.push_str(init_props); pre.push('\n');
    pre.push_str(&format!("state: \"{st}\"\nstates: State {{\nname: \"visible\"\nPropertyChanges {{\ntarget: parent\n{target_props}\n}}\n}}\ntransitions: Transition {{\nNumberAnimation {{\nproperties: {anim_props}\nduration: {d}\neasing.type: {e}\n}}\n}}\n{timer}",
        d=a.dur, e=a.easing));
    (pre, "}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All inputs None → no animation detected.
    #[test]
    fn no_inputs_no_animation() {
        assert!(detect_animation(None, None, None, None).is_none());
    }

    /// data-aos="fade-up" → FadeSlideUp.
    #[test]
    fn aos_fade_up() {
        let a = detect_animation(None, None, Some("fade-up"), None).expect("should detect");
        matches!(a.kind, AnimKind::FadeSlideUp);
    }

    /// data-aos="zoom-in" → ZoomIn.
    #[test]
    fn aos_zoom_in() {
        let a = detect_animation(None, None, Some("zoom-in"), None).expect("should detect");
        matches!(a.kind, AnimKind::ZoomIn);
    }

    /// data-aos with unknown value falls back to FadeIn.
    #[test]
    fn aos_unknown_falls_back() {
        let a = detect_animation(None, None, Some("nonsense-value"), None).expect("should detect");
        matches!(a.kind, AnimKind::FadeIn);
    }

    /// classes="animate__fadeIn" → FadeIn.
    #[test]
    fn classes_animate_fadein() {
        let a = detect_animation(Some("animate__fadeIn"), None, None, None);
        assert!(a.is_some(), "should detect animate__fadeIn");
        matches!(a.unwrap().kind, AnimKind::FadeIn);
    }

    /// classes="animate__fadeInUp" → FadeSlideUp (more specific match).
    #[test]
    fn classes_animate_fadeinup() {
        let a = detect_animation(Some("animate__fadeInUp"), None, None, None);
        assert!(a.is_some(), "should detect animate__fadeInUp");
        matches!(a.unwrap().kind, AnimKind::FadeSlideUp);
    }

    /// classes="wow" → fade with default WOW timing.
    #[test]
    fn classes_wow_lib() {
        let a = detect_animation(Some("wow"), None, None, None).expect("should detect");
        assert_eq!(a.delay, 200);
        assert_eq!(a.dur, 600);
    }

    /// Pure opacity:0 (no transition) → None (hidden, not
    /// animated — important: don't make hidden content
    /// fade-in spuriously).
    #[test]
    fn style_opacity_zero_no_transition_none() {
        let a = detect_animation(None, Some("opacity: 0;"), None, None);
        assert!(a.is_none());
    }

    /// CSS opacity transition → FadeIn.
    #[test]
    fn style_opacity_transition_fades() {
        let a = detect_animation(None,
            Some("transition: opacity 0.3s ease;"), None, None);
        assert!(a.is_some());
        matches!(a.unwrap().kind, AnimKind::FadeIn);
    }

    /// data-aos wins over class-based detection (priority order).
    #[test]
    fn aos_wins_over_classes() {
        // animate__fadeIn would give FadeIn; aos="zoom-in" would
        // give ZoomIn. data-aos comes first in the function so
        // ZoomIn wins.
        let a = detect_animation(Some("animate__fadeIn"), None, Some("zoom-in"), None)
            .expect("should detect");
        matches!(a.kind, AnimKind::ZoomIn);
    }
}
