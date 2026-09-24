use super::*;

fn s(x: f64) -> Ticks { Ticks::from_seconds(x) }
fn seg(a: f64, b: f64, text: &str) -> Segment {
    Segment { start: s(a), end: s(b), text: text.into(), speaker: None, confidence: 0.95 }
}
fn take(id: u64, name: &str, segs: Vec<Segment>) -> Take {
    Take { source: SourceId(id), name: name.into(), segments: segs, len: s(30.0), angle_group: None }
}
fn no_noise(_: &str) -> bool { false }
fn outline(lines: &[&str]) -> Vec<String> { lines.iter().map(|l| l.to_string()).collect() }

/// A film take: slate, action, two lines, cut, chatter.
fn film_take(id: u64, name: &str, first: &str, second: &str) -> Take {
    take(id, name, vec![
        seg(0.5, 2.0, "Camera rolling. Scene 13, take 2."),
        seg(2.5, 3.0, "Action!"),
        seg(4.0, 6.0, first),
        seg(7.0, 9.0, second),
        seg(10.0, 10.5, "Cut!"),
        seg(11.0, 13.0, "That was great, let's go again."),
    ])
}

#[test]
fn cleaning_keeps_only_the_performance_window_and_drops_slate_and_noise() {
    let mut t = film_take(1, "C1", "You like acting, come on.", "Thank you.");
    t.segments.insert(3, seg(6.2, 6.8, "scene 13 take 2"));
    let is_noise = |x: &str| x.trim() == "Thank you.";
    let lines = clean(&t, &is_noise);
    let why: Vec<_> = lines.iter().map(|l| (l.text.as_str(), l.dropped)).collect();
    assert_eq!(why, vec![
        ("Camera rolling. Scene 13, take 2.", Some(Dropped::OutsideWindow)),
        ("Action!", Some(Dropped::OutsideWindow)),
        ("You like acting, come on.", None),
        ("scene 13 take 2", Some(Dropped::Slate)),
        ("Thank you.", Some(Dropped::Noise)),
        ("Cut!", Some(Dropped::OutsideWindow)),
        ("That was great, let's go again.", Some(Dropped::OutsideWindow)),
    ]);
    // a restarted take: the window opens after the LAST action
    let r = take(2, "C2", vec![seg(0.0, 1.0, "Action"), seg(1.5, 2.0, "Sorry, sorry."), seg(3.0, 3.5, "Acción"),
                               seg(4.0, 5.0, "The real line."), seg(6.0, 6.5, "Corte")]);
    let kept: Vec<_> = clean(&r, &no_noise).into_iter().filter(|l| l.dropped.is_none()).map(|l| l.text).collect();
    assert_eq!(kept, vec!["The real line."]);
    // an interview never calls action: all of it is performance
    let i = take(3, "I", vec![seg(0.0, 2.0, "So tell me about the first show."), seg(3.0, 5.0, "It was a disaster.")]);
    assert!(clean(&i, &no_noise).iter().all(|l| l.dropped.is_none()));
}

#[test]
fn similarity_is_word_order_aware_and_ignores_case_and_punctuation() {
    assert_eq!(similarity("You like acting, come on.", "you like ACTING come on"), 1.0);
    let swapped = similarity("you like acting come on", "come on you like acting");
    assert!(swapped < 0.7 && swapped > 0.5, "{swapped}");
    assert_eq!(similarity("", "anything"), 0.0);
}

#[test]
fn an_outline_line_is_found_in_the_take_that_says_it_with_the_others_as_alternatives() {
    let takes = vec![
        film_take(1, "C1", "You like acting, come on.", "Partying is great."),
        film_take(2, "C2", "You like acting? Come on!", "Partying is so great."),
    ];
    let o = outline(&["Partying is great.", "You like acting, come on."]);
    let a = assemble(&takes, &o, &Options::default(), &no_noise, s(0.0));
    assert_eq!(a.placements.len(), 2);
    // outline order, not take order
    assert_eq!(a.placements[0].source, SourceId(1), "C1 says it exactly: {:?}", a.placements[0]);
    assert_eq!(a.placements[0].outline_line, Some(0));
    assert_eq!(a.placements[0].alternatives[0].0, SourceId(2));
    assert!(a.placements[0].reason.contains("next best C2"), "{}", a.placements[0].reason);
    // the two readings of line 2 are word-identical: a close call between performances
    assert!(a.reviews.iter().any(|r| matches!(r, Review::CloseCall { placement: 1, .. })), "{:?}", a.reviews);
    // end to end, from the start time
    assert_eq!(a.placements[1].at, a.placements[0].end());
}

#[test]
fn the_callers_preference_breaks_a_tie_and_angles_of_one_performance_are_no_close_call() {
    let mut takes = vec![film_take(1, "C1", "Hello there.", "Bye now."), film_take(2, "C2", "Hello there.", "Bye now.")];
    let o = outline(&["Hello there."]);
    let mut opts = Options::default();
    opts.prefer.insert(SourceId(1), 0.05);
    let a = assemble(&takes, &o, &opts, &no_noise, s(0.0));
    assert_eq!(a.placements[0].source, SourceId(1), "preferred");
    assert!(a.reviews.iter().any(|r| matches!(r, Review::CloseCall { .. })), "still close: two performances");
    takes[0].angle_group = Some(7);
    takes[1].angle_group = Some(7);
    let a = assemble(&takes, &o, &opts, &no_noise, s(0.0));
    assert!(!a.reviews.iter().any(|r| matches!(r, Review::CloseCall { .. })), "angles: {:?}", a.reviews);
}

#[test]
fn a_line_whisper_split_in_two_is_found_as_one_run_and_a_missing_line_is_reported() {
    let t = take(1, "I", vec![seg(1.0, 2.0, "I never wanted"), seg(2.1, 3.5, "to be famous, you know."),
                              seg(5.0, 6.0, "Next question.")]);
    let o = outline(&["I never wanted to be famous, you know.", "Where were you born?"]);
    let a = assemble(&[t], &o, &Options::default(), &no_noise, s(0.0));
    assert_eq!(a.placements.len(), 1);
    let p = &a.placements[0];
    assert_eq!(p.text, "I never wanted to be famous, you know.");
    assert!(p.src_in < s(1.0) && p.src_out > s(3.5) && p.src_out < s(5.0), "{p:?}");
    assert!(matches!(&a.reviews[0], Review::Missing { outline_line: 1, .. }), "{:?}", a.reviews);
}

#[test]
fn handles_add_air_but_never_reach_into_the_neighbouring_words() {
    let t = take(1, "I", vec![seg(1.0, 2.0, "Before."), seg(2.2, 4.0, "The line we want here."), seg(4.1, 5.0, "After.")]);
    let a = assemble(&[t], &outline(&["The line we want here."]), &Options::default(), &no_noise, s(10.0));
    let p = &a.placements[0];
    // 0.25 s before would be 1.95, inside "Before." -- stops halfway, at 2.1
    assert!((p.src_in.to_seconds() - 2.1).abs() < 1e-6, "{:?}", p.src_in);
    assert!((p.src_out.to_seconds() - 4.05).abs() < 1e-6, "{:?}", p.src_out);
    assert_eq!(p.at, s(10.0));
}

#[test]
fn without_an_outline_takes_run_in_name_order_and_a_line_said_again_keeps_the_last_saying() {
    let takes = vec![
        take(2, "C0002", vec![seg(1.0, 3.0, "I'm not going back there."), seg(4.0, 6.0, "Not after what happened.")]),
        take(1, "C0001", vec![seg(1.0, 3.0, "I am not going back there."), seg(4.0, 5.0, "Ever.")]),
    ];
    let a = assemble(&takes, &[], &Options::default(), &no_noise, s(0.0));
    let got: Vec<_> = a.placements.iter().map(|p| (p.source, p.text.as_str())).collect();
    assert_eq!(got, vec![(SourceId(2), "I'm not going back there."), (SourceId(1), "Ever."), (SourceId(2), "Not after what happened.")]);
    assert_eq!(a.placements[0].alternatives[0].0, SourceId(1), "the replaced saying is an alternative");
    assert!(a.placements[0].reason.contains("C0002"), "{}", a.placements[0].reason);
}

#[test]
fn numbers_doubtful_words_and_clipped_lines_go_on_the_review_list() {
    let mut t = take(1, "I", vec![seg(1.0, 3.0, "We open on the 14th."), seg(4.0, 4.2, "Go."), seg(6.0, 8.0, "Something murky.")]);
    t.segments[2].confidence = 0.3;
    let a = assemble(&[t], &outline(&["We open on the 14th.", "Go.", "Something murky."]), &Options::default(), &no_noise, s(0.0));
    let kinds: Vec<_> = a.reviews.iter().map(|r| std::mem::discriminant(r)).collect();
    assert!(a.reviews.iter().any(|r| matches!(r, Review::Number { placement: 0 })), "{:?}", a.reviews);
    assert!(a.reviews.iter().any(|r| matches!(r, Review::Doubtful { placement: 2, .. })));
    assert!(!a.reviews.iter().any(|r| matches!(r, Review::Short { placement: 1, .. })), "0.2 s + air is 0.8 s: fine");
    assert_eq!(kinds.len(), 2);
}

#[test]
fn the_ops_lay_one_clip_per_placement_and_one_uniquely_named_marker_per_review() {
    let takes = vec![film_take(1, "C1", "Line one.", "Line two."), film_take(2, "C2", "Line one.", "Line two.")];
    let a = assemble(&takes, &outline(&["Line one.", "A line nobody says.", "Line two."]), &Options::default(), &no_noise, s(2.0));
    let names: BTreeMap<SourceId, String> = [(SourceId(1), "C1".into()), (SourceId(2), "C2".into())].into();
    let ops = a.ops(TrackId(9), 100, &names);
    let clips: Vec<_> = ops.iter().filter_map(|o| match o { Op::AddClip { clip } => Some(clip.clone()), _ => None }).collect();
    assert_eq!(clips.len(), 2);
    assert_eq!((clips[0].id, clips[0].track, clips[0].pos), (ClipId(100), TrackId(9), s(2.0)));
    let markers: Vec<_> = ops.iter().filter_map(|o| match o { Op::SetMarker { at, name } => Some((*at, name.clone())), _ => None }).collect();
    assert_eq!(markers.len(), a.reviews.len());
    let mut unique: Vec<_> = markers.iter().map(|m| &m.1).collect();
    unique.dedup();
    assert_eq!(unique.len(), markers.len());
    // the missing line is marked where it would have gone: after line one
    let missing = markers.iter().find(|m| m.1.contains("missing")).expect("a marker for the missing line");
    assert_eq!(missing.0, a.placements[0].end(), "{markers:?}");
    assert!(a.summary().contains("1 outline line not found"), "{}", a.summary());
}

#[test]
fn outline_text_loses_speaker_tags_comments_and_blank_lines() {
    let o = parse_outline("# scene N7\nMAX: Yeah, I found something I like more than that.\n\nJude: You like acting, come on.\nA line: with a colon inside the words, not a speaker.");
    assert_eq!(o, vec!["Yeah, I found something I like more than that.", "You like acting, come on.",
                       "A line: with a colon inside the words, not a speaker."]);
}

/// The cartoon-fragile fixture: three cameras of ONE performance, each with
/// its own `.srt` in its own time. Every line is found, on one camera, with
/// no close calls (they are angles), laid end to end in outline order.
#[test]
fn the_fragile_fixture_assembles_every_line_without_calling_angles_a_choice() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cartoon-fragile");
    let takes: Vec<Take> = ["A001_wide", "A002_close_left", "A003_close_right"].iter().enumerate().map(|(i, n)| {
        let srt = std::fs::read_to_string(dir.join(format!("{n}.srt"))).expect("fixture srt");
        Take { source: SourceId(i as u64 + 1), name: n.to_string(), segments: studio_voice::parse_srt(&srt),
               len: s(14.0), angle_group: Some(1) }
    }).collect();
    assert!(takes.iter().all(|t| t.segments.len() == 4), "the fixture's four lines");
    let o = parse_outline("MAX: Yeah, I found something I like more than that.\nJUDE: You like acting, come on.\n\
                           MAX: Partying is great.\nWOMAN: Hi guys, mind if we join the party?");
    let a = assemble(&takes, &o, &Options::default(), &no_noise, s(0.0));
    assert_eq!(a.placements.len(), 4, "{}", a.summary());
    assert!(a.reviews.is_empty(), "{:?}", a.reviews);
    for (i, p) in a.placements.iter().enumerate() {
        assert_eq!(p.outline_line, Some(i));
        assert_eq!(p.alternatives.len(), 2, "the other two angles");
        if i > 0 { assert_eq!(p.at, a.placements[i - 1].end()); }
    }
    // the known line times: Jude's line runs 5.0-7.0 on the wide (A001)
    let jude = &a.placements[1];
    let seg = &takes[(jude.source.0 - 1) as usize].segments[1];
    assert!(jude.src_in <= seg.start && jude.src_out >= seg.end, "{jude:?} vs {seg:?}");
}
