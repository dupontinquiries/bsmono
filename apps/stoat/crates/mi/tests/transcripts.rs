use stoat_mi::*;

fn records(name: &str) -> Vec<Record> {
    let path = format!("{}/tests/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).unwrap().split(|b| *b == b'\n').filter(|l| !l.is_empty()).map(parse_line).collect()
}

#[test]
fn crash_transcript_parses_completely() {
    let recs = records("crash_gdb15_mi3.txt");
    assert!(!recs.iter().any(|r| matches!(r, Record::Unparsed(_))));

    let stop = recs
        .iter()
        .find_map(|r| match r {
            Record::Async { kind: AsyncKind::Exec, class, results, .. } if class == "stopped" => Some(results),
            _ => None,
        })
        .unwrap();
    assert_eq!(stop.get_str("reason"), Some("signal-received"));
    assert_eq!(stop.get_str("signal-name"), Some("SIGSEGV"));
    let frame = stop.get("frame").unwrap();
    assert_eq!(frame.get_str("func"), Some("depth3"));
    assert_eq!(frame.get_u64("line"), Some(6));
    assert!(frame.get_u64("addr").unwrap() > 0x5555_0000_0000);

    let frames = recs
        .iter()
        .find_map(|r| match r {
            Record::Result { token: Some(1), results, .. } => results.get("stack"),
            _ => None,
        })
        .unwrap();
    let funcs: Vec<_> = frames.items().filter_map(|f| f.get_str("func")).collect();
    assert_eq!(funcs, ["depth3", "depth2", "depth1", "main"]);

    let err = recs
        .iter()
        .find_map(|r| match r {
            Record::Result { token: Some(4), class, results } if class == "error" => results.get_str("msg"),
            _ => None,
        })
        .unwrap();
    assert_eq!(err, "No symbol \"nosuch\" in current context.");
}

#[test]
fn mi2_bare_tuples_in_result_position() {
    let recs = records("multiloc_gdb15_mi2.txt");
    assert!(!recs.iter().any(|r| matches!(r, Record::Unparsed(_))));
    let bp = recs
        .iter()
        .find_map(|r| match r {
            Record::Result { class, results, .. } if class == "done" && results.get("bkpt").is_some() => Some(results),
            _ => None,
        })
        .unwrap();
    assert_eq!(bp.get("bkpt").unwrap().get_str("addr"), Some("<MULTIPLE>"));
    // The locations follow as unnamed tuples.
    let locs: Vec<_> = bp.items().skip(1).filter_map(|v| v.get_str("number")).collect();
    assert_eq!(locs, ["1.1", "1.2"]);
}

#[test]
fn half_escaped_utf8() {
    // em dash E2 80 94: first byte raw, next two octal.
    let mut raw = b"~\"x ".to_vec();
    raw.push(0xE2);
    raw.extend_from_slice(b"\\200\\224 y\"");
    match parse_line(&raw) {
        Record::Stream { text, .. } => assert_eq!(text, "x \u{2014} y"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn junk_is_unparsed_not_fatal() {
    for l in ["hello", "^", "^done,", "^done,x", "^done,x={", "~\"unterminated", "12", "=x,y=[\"a\",]"] {
        assert!(matches!(parse_str(l), Record::Unparsed(_)), "{l:?}");
    }
    assert_eq!(parse_str("(gdb) \r"), Record::Prompt);
    assert!(matches!(parse_str("^done,a={},b=[]"), Record::Result { .. }));
}

#[test]
fn command_encoding() {
    let c = Command::new("-break-insert").opt("-f").arg("src/main.rs:12");
    assert_eq!(c.encode(7), "7-break-insert -f src/main.rs:12\n");
    let c = Command::new("exec-arguments").arg("--name").arg("two words").arg("");
    assert_eq!(c.encode(1), "1-exec-arguments \"--name\" \"two words\" \"\"\n");
    let c = Command::new("interpreter-exec").opt("console").quoted("p \"x\\n\"");
    assert_eq!(c.encode(2), "2-interpreter-exec console \"p \\\"x\\\\n\\\"\"\n");
}
