use stoat_core::backend::{Backend, ReqId, Request, StepKind};
use stoat_core::command::{catalog, interpret_spoken, parse, Command};

#[test]
fn typed_lines_parse_like_the_gdb_abbreviations_they_borrow() {
    assert_eq!(parse("c"), Some(Command::Continue));
    assert_eq!(parse(" next "), Some(Command::Step(StepKind::Over)));
    assert_eq!(parse("fin"), Some(Command::Step(StepKind::Out)));
    assert_eq!(parse("up"), Some(Command::Up(1)));
    assert_eq!(parse("u 3"), Some(Command::Up(3)));
    assert_eq!(parse("f 2"), Some(Command::Frame(2)));
    assert_eq!(parse("p n->next->v + 1"), Some(Command::Print("n->next->v + 1".into())));
    assert_eq!(parse("b src/main.rs:12"), Some(Command::Break("src/main.rs:12".into())));
    assert_eq!(parse("! info sharedlibrary"), Some(Command::Raw("info sharedlibrary".into())));
    assert_eq!(parse("attach 4242"), Some(Command::Attach(4242)));
    assert_eq!(parse("core /tmp/core.1"), Some(Command::OpenCore("/tmp/core.1".into())));
    // Not commands: these fall through to search in a palette.
    for l in ["", "vitals", "c x", "up two", "p", "b", "!", "frame", "continue please"] {
        assert_eq!(parse(l), None, "{l:?}");
    }
}

#[test]
fn speech_is_matched_by_phrase_with_recogniser_noise_tolerated() {
    let scope = vec!["counter".to_string(), "my_var".to_string(), "doubled".to_string()];
    let s = |u: &str| interpret_spoken(u, &scope);
    assert_eq!(s("Step over."), Some(Command::Step(StepKind::Over)));
    assert_eq!(s("okay, keep going"), Some(Command::Continue));
    assert_eq!(s("Step into"), Some(Command::Step(StepKind::Into)));
    assert_eq!(s("go up two frames"), Some(Command::Up(2)));
    assert_eq!(s("up to"), Some(Command::Up(2)), "'two' is routinely heard as 'to'");
    assert_eq!(s("down 3"), Some(Command::Down(3)));
    assert_eq!(s("frame number four"), Some(Command::Frame(4)));
    assert_eq!(s("print my var"), Some(Command::Print("my_var".into())), "resolved against scope");
    assert_eq!(s("what is counters?"), Some(Command::Print("counter".into())), "one letter off still resolves");
    assert_eq!(s("print tile cache"), Some(Command::Print("tile_cache".into())), "unknown names fall back to snake_case");
    assert_eq!(s("show doubled"), Some(Command::Print("doubled".into())));
    assert_eq!(s("show me the vitals page"), None, "'show' is also UI talk; it prints only what is in scope");
    assert_eq!(s("break at main"), Some(Command::Break("main".into())));
    assert_eq!(s("break at line forty"), None, "only number words it knows; a wrong line is worse than none");
    assert_eq!(s("break at line 40"), Some(Command::Break("40".into())));
    for u in ["", "hello there", "can't you continue from here", "print", "up banana"] {
        assert_eq!(s(u), None, "{u:?}");
    }
}

#[test]
fn everything_that_lets_the_program_run_is_marked_so() {
    let resumes: Vec<String> = catalog().into_iter().map(|(c, _)| c).filter(Command::resumes_target).map(|c| c.label()).collect();
    assert_eq!(resumes, ["step over", "continue", "step into", "step out", "run / restart"]);
    for c in [Command::Up(1), Command::Print("x".into()), Command::Break("main".into()), Command::Interrupt, Command::Frame(0)] {
        assert!(!c.resumes_target(), "{c:?}");
    }
    assert!(Command::Attach(1).is_loader() && Command::Build.is_loader() && !Command::Continue.is_loader());
}

#[test]
fn the_driver_executes_session_commands_and_hands_loaders_back() {
    use std::sync::{Arc, Mutex};
    use stoat_core::driver::Driver;
    use stoat_core::session::Session;
    use stoat_core::vitals::Vitals;
    #[derive(Default, Clone)]
    struct Rec(Arc<Mutex<Vec<Request>>>);
    impl Backend for Rec {
        fn submit(&mut self, req: Request) -> ReqId {
            let mut v = self.0.lock().unwrap();
            v.push(req);
            v.len() as u64
        }
    }
    let rec = Rec::default();
    let mut d = Driver::new(Session::new(None), Box::new(rec.clone()), Vitals::shared(4, 4));
    assert!(d.execute(&Command::Step(StepKind::Over)).is_ok());
    assert!(d.execute(&Command::Break("main".into())).is_ok());
    assert!(d.execute(&Command::Raw("info line".into())).is_ok());
    assert!(d.execute(&Command::Attach(7)).is_err(), "a loader is the shell's to handle");
    assert_eq!(
        *rec.0.lock().unwrap(),
        [Request::Step { kind: StepKind::Over, thread: None }, Request::InsertBreakpoint { location: "main".into() }, Request::Raw("info line".into())]
    );
}

#[test]
fn the_typed_form_round_trips_for_every_kind_of_command() {
    let all = [
        Command::Continue, Command::Interrupt, Command::Run,
        Command::Step(StepKind::Over), Command::Step(StepKind::Into), Command::Step(StepKind::Out),
        Command::Up(1), Command::Down(4), Command::Frame(0), Command::Thread(3),
        Command::Print("a->b + 1".into()), Command::Break("src/main.rs:12".into()), Command::Raw("info line".into()),
        Command::Build, Command::Debug("tile-viewer".into()), Command::Attach(99), Command::OpenCore("/tmp/core".into()),
    ];
    for c in all {
        assert_eq!(parse(&c.typed()), Some(c.clone()), "{:?} -> {:?}", c, c.typed());
    }
}
