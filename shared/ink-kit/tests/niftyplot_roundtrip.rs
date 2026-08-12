//! The contract that matters for the voiceterm→logscope path: the niftyplot
//! text ink-kit emits must parse in the *real* `nifty-core` parser into the
//! same nodes and edges we recognized. Path dev-dependency; runs offline.

use ink_kit::recognize::{recognize_graph, ShapeClass};
use ink_kit::stroke::{InkPoint, Stroke, StrokeId, StrokeStyle};

fn stroke_from(pts: &[(f64, f64)]) -> Stroke {
    let mut s = Stroke::new(StrokeId(0), StrokeStyle::default());
    s.points = pts.iter().map(|&(x, y)| InkPoint::xy(x, y)).collect();
    s
}

fn rect(cx: f64, cy: f64, w: f64, h: f64) -> Stroke {
    let (x0, y0, x1, y1) = (cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0);
    let mut pts = Vec::new();
    let n = 10;
    for i in 0..=n {
        pts.push((x0 + (x1 - x0) * i as f64 / n as f64, y0));
    }
    for i in 0..=n {
        pts.push((x1, y0 + (y1 - y0) * i as f64 / n as f64));
    }
    for i in 0..=n {
        pts.push((x1 - (x1 - x0) * i as f64 / n as f64, y1));
    }
    for i in 0..=n {
        pts.push((x0, y1 - (y1 - y0) * i as f64 / n as f64));
    }
    stroke_from(&pts)
}

fn circle(cx: f64, cy: f64, r: f64) -> Stroke {
    stroke_from(
        &(0..=64)
            .map(|i| {
                let a = i as f64 / 64.0 * std::f64::consts::TAU;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect::<Vec<_>>(),
    )
}

#[test]
fn sketch_exports_and_reparses_in_nifty_core() {
    // Three boxes: top-left rect, top-right circle, bottom rect.
    let a = rect(0.0, 0.0, 100.0, 60.0);
    let b = circle(300.0, 0.0, 40.0);
    let c = rect(150.0, 250.0, 120.0, 60.0);
    // a -> b, a -> c (arrows drawn from inside source toward target)
    let ab = stroke_from(&[(45.0, 0.0), (150.0, 2.0), (255.0, 0.0)]);
    let ac = stroke_from(&[(10.0, 25.0), (60.0, 120.0), (120.0, 215.0)]);

    let sketch = recognize_graph(&[a, b, c, ab, ac], 0.35);
    assert_eq!(sketch.nodes.len(), 3);
    assert_eq!(sketch.edges.len(), 2);
    assert_eq!(sketch.nodes[1].class, ShapeClass::Ellipse);

    // Attach a label the way a labeling pass (or user edit) would.
    let mut labeled = sketch.clone();
    labeled.nodes[0].label = Some("source \"quoted\"".to_string());

    let text = labeled.to_niftyplot();
    let doc = nifty_core::niftyplot::parse(&text);

    assert_eq!(doc.nodes.len(), 3, "source:\n{}\nwarnings: {:?}", text, doc.warnings);
    assert_eq!(doc.edges.len(), 2, "source:\n{}", text);

    // Node ids survive; the circle keeps its shape hint; the label round-trips.
    let n1 = doc.nodes.iter().find(|n| n.id == "n1").expect("n1 present");
    assert_eq!(n1.label.as_deref(), Some("source \"quoted\""));
    let n2 = doc.nodes.iter().find(|n| n.id == "n2").expect("n2 present");
    assert_eq!(n2.shape, nifty_core::niftyplot::Shape::Circle);

    // Both edges leave n1 (reading order puts the top-left rect first).
    for e in &doc.edges {
        assert_eq!(e.from, "n1");
    }
    let targets: Vec<&str> = doc.edges.iter().map(|e| e.to.as_str()).collect();
    assert!(targets.contains(&"n2") && targets.contains(&"n3"), "{:?}", targets);
}
