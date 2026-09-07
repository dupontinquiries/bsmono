//! engine_knobs — the preflight overrides, as something you can turn.
//!
//! The engine reads a handful of `LOGSCOPE_*` environment variables at model load and the
//! preflight summary reports which ones took. That is fine for a developer with a shell and
//! useless for anyone else: to try `LOGSCOPE_MATVEC_SG=1` you must know the variable exists,
//! know its spelling, and restart the app from a terminal.
//!
//! This is the same set, described: what each one does, what values are sensible, and
//! whether changing it takes effect now or at the next load.
//!
//! ## Env wins, and the UI says so
//!
//! A knob set in the environment overrides whatever the UI holds. The alternative — the UI
//! silently winning — would mean a shell override that appears to be honoured and is not,
//! which is the exact failure `backend.rs` already warns about in the preflight code:
//!
//! > A preflight summary that reports intent rather than outcome is worse than none: it is
//! > read as confirmation.
//!
//! A toggle that cannot take effect is the same lie in a different place, so
//! [`Resolved::source`] carries where the value came from and the panel is expected to show
//! it and disable the control. There is no third option where the UI quietly loses.
//!
//! ## "Within reason"
//!
//! Every numeric knob has a clamped range, and the clamp is part of the knob rather than a
//! check at the call site. These are dispatch sizes and submit counts on a watchdog-bounded
//! GPU: a prefill chunk of 100,000 does not produce a slow run, it produces a device-lost
//! and a driver reset. The ranges below are wide enough to explore and narrow enough that
//! the worst outcome is a bad frame rate.
//!
//! Two knobs are marked [`Risk::Fragile`]: they turn OFF a mitigation that exists because
//! the hardware needed it. They are still exposed — the whole point is experimenting — but
//! a caller is expected to mark them, because "this is the one that hangs Asahi" is
//! information the operator has no other way to get.

/// What a knob's value can be.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Flag(bool),
    /// A count, clamped to the knob's range.
    Count(u32),
}

impl Value {
    /// The string the engine's `std::env::var` reads.
    pub fn as_env(&self) -> String {
        match self {
            Value::Flag(b) => (if *b { "1" } else { "0" }).to_string(),
            Value::Count(n) => n.to_string(),
        }
    }
}

/// How much trouble a knob can cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Changes speed. The worst case is a slower run.
    Tuning,
    /// Turns off a mitigation that exists because some hardware needed it. The worst case
    /// is a hang or a device-lost, recoverable by clearing the knob and reloading.
    Fragile,
}

/// When a change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applies {
    /// Read at model load. Changing it needs a reload, and a UI that does not say so
    /// invites the operator to conclude the knob does nothing.
    OnLoad,
}

/// One tunable.
#[derive(Debug, Clone)]
pub struct Knob {
    /// The environment variable the engine reads.
    pub env: &'static str,
    /// An older spelling still honoured. `PREFILL_CHUNK` vs `LOGSCOPE_PREFILL_CHUNK` is
    /// named in `backend.rs` as a silent-ignore that cost real test cycles.
    pub alias: Option<&'static str>,
    pub label: &'static str,
    /// One line, in the operator's terms: what it does and what to expect.
    pub help: &'static str,
    pub default: Value,
    /// Inclusive range for a `Count`. Ignored for a flag.
    pub range: (u32, u32),
    pub risk: Risk,
    pub applies: Applies,
}

impl Knob {
    /// Clamp a value into what this knob accepts. A count outside the range is brought to
    /// the nearest end rather than rejected: a slider cannot produce one, but a settings
    /// file written by hand or by an older build can.
    pub fn clamp(&self, v: Value) -> Value {
        match v {
            Value::Flag(_) if matches!(self.default, Value::Count(_)) => self.default.clone(),
            Value::Count(_) if matches!(self.default, Value::Flag(_)) => self.default.clone(),
            Value::Count(n) => Value::Count(n.clamp(self.range.0, self.range.1)),
            f => f,
        }
    }

    /// Parse a value as it would arrive from the environment. `None` for something this
    /// knob cannot use — which is worth surfacing, because a mistyped env var currently
    /// looks identical to an absent one.
    pub fn parse(&self, s: &str) -> Option<Value> {
        let s = s.trim();
        match self.default {
            Value::Flag(_) => match s.to_ascii_lowercase().as_str() {
                "1" | "true" | "on" | "yes" => Some(Value::Flag(true)),
                "0" | "false" | "off" | "no" => Some(Value::Flag(false)),
                _ => None,
            },
            Value::Count(_) => s.parse::<u32>().ok().map(|n| self.clamp(Value::Count(n))),
        }
    }
}

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The knob's own default — nothing has been set.
    Default,
    /// The operator set it in this panel.
    Ui,
    /// The environment set it, and it wins. The control should be shown disabled with the
    /// variable named, not silently ignored.
    Env,
    /// The environment set it to something this knob cannot parse. It does NOT win —
    /// nothing can be done with it — and saying so is the only way an operator finds a
    /// typo.
    EnvUnparsed,
}

/// A knob's effective value and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub value: Value,
    pub source: Source,
    /// The raw string, when the environment supplied one. Shown as-is for the unparsed case.
    pub raw: Option<String>,
}

/// The knobs the preflight summary already reports on.
pub fn all() -> Vec<Knob> {
    vec![
        Knob {
            env: "LOGSCOPE_MATVEC_SG",
            alias: None,
            label: "Subgroup matvec",
            help: "Faster matrix-vector kernels using GPU subgroups. Turned OFF automatically on \
                   Asahi/Honeykrisp, where they have hung. Turn on to measure it on your machine; \
                   if the decode stalls, turn it back off and reload.",
            default: Value::Flag(false),
            range: (0, 1),
            risk: Risk::Fragile,
            applies: Applies::OnLoad,
        },
        Knob {
            env: "LOGSCOPE_WINDOW_ONE_SUBMIT",
            alias: None,
            label: "One submit per decode window",
            help: "Submits a whole decode window in one go instead of bounded pieces. Fewer \
                   round trips, and on a watchdog-bounded driver a long submit is what the \
                   watchdog kills.",
            default: Value::Flag(false),
            range: (0, 1),
            risk: Risk::Fragile,
            applies: Applies::OnLoad,
        },
        Knob {
            env: "LOGSCOPE_DECODE_ATTN_SERIAL",
            alias: None,
            label: "Serial decode attention",
            help: "Runs decode attention one head at a time. Slower, and the thing to try when \
                   output is wrong rather than slow — it removes the parallel path from the \
                   picture.",
            default: Value::Flag(false),
            range: (0, 1),
            risk: Risk::Tuning,
            applies: Applies::OnLoad,
        },
        Knob {
            env: "LOGSCOPE_PREFILL_CHUNK",
            alias: Some("PREFILL_CHUNK"),
            label: "Prefill chunk (tokens)",
            help: "How many tokens are prefilled per dispatch. Larger is faster until a dispatch \
                   outlives the driver's watchdog; smaller is safer and adds overhead.",
            default: Value::Count(256),
            range: (16, 2048),
            risk: Risk::Tuning,
            applies: Applies::OnLoad,
        },
        Knob {
            env: "LOGSCOPE_FLUSH_EVERY",
            alias: None,
            label: "Flush every N submits",
            help: "How often the queue is flushed. Low values cost throughput and make a hang \
                   easier to localise; high values are faster and blur where it stopped.",
            default: Value::Count(16),
            range: (1, 256),
            risk: Risk::Tuning,
            applies: Applies::OnLoad,
        },
    ]
}

/// Resolve one knob against the environment and the panel's stored value.
///
/// `env_lookup` is passed in so this is testable without touching the process environment —
/// and so a caller can resolve against a snapshot rather than racing a `setenv`.
pub fn resolve(
    knob: &Knob,
    ui_value: Option<&Value>,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> Resolved {
    let raw = env_lookup(knob.env).or_else(|| knob.alias.and_then(&env_lookup));
    if let Some(r) = raw {
        return match knob.parse(&r) {
            Some(v) => Resolved { value: v, source: Source::Env, raw: Some(r) },
            // Unparsed: the engine's own `var` read will also fail to make sense of it, so
            // the effective value is the UI's or the default. Reported, because a typo in
            // an env var is otherwise indistinguishable from not setting it.
            None => Resolved {
                value: ui_value.cloned().unwrap_or_else(|| knob.default.clone()),
                source: Source::EnvUnparsed,
                raw: Some(r),
            },
        };
    }
    match ui_value {
        Some(v) => Resolved { value: knob.clamp(v.clone()), source: Source::Ui, raw: None },
        None => Resolved { value: knob.default.clone(), source: Source::Default, raw: None },
    }
}

/// Is this control the operator's to move? `false` when the environment owns it.
pub fn editable(r: &Resolved) -> bool {
    r.source != Source::Env
}

/// The `(name, value)` pairs to set before loading a model, for the knobs the UI owns.
///
/// Deliberately excludes anything the environment already set: writing over it would make
/// the shell override disappear, which is the opposite of what an override is for. Also
/// excludes values equal to the default, so an untouched panel changes nothing.
pub fn env_to_apply(
    ui: &[(&'static str, Value)],
    env_lookup: impl Fn(&str) -> Option<String> + Copy,
) -> Vec<(&'static str, String)> {
    let knobs = all();
    let mut out = Vec::new();
    for (name, v) in ui {
        let Some(k) = knobs.iter().find(|k| k.env == *name) else { continue };
        let r = resolve(k, Some(v), env_lookup);
        if r.source == Source::Env || r.value == k.default {
            continue;
        }
        out.push((k.env, r.value.as_env()));
    }
    out
}

/// A one-line summary for the panel header: how many knobs are off their default, and how
/// many of those the environment owns.
pub fn summary(resolved: &[(&Knob, Resolved)]) -> String {
    let changed = resolved.iter().filter(|(k, r)| r.value != k.default).count();
    let env = resolved.iter().filter(|(_, r)| r.source == Source::Env).count();
    let bad = resolved.iter().filter(|(_, r)| r.source == Source::EnvUnparsed).count();
    let mut s = if changed == 0 { "all default".to_string() } else { format!("{changed} changed") };
    if env > 0 {
        s.push_str(&format!(" · {env} from the environment"));
    }
    if bad > 0 {
        s.push_str(&format!(" · {bad} unreadable"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knob(env: &str) -> Knob {
        all().into_iter().find(|k| k.env == env).expect("knob exists")
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    /// The environment wins, and the panel is told so rather than showing a toggle that
    /// does nothing. A UI that silently overrode a shell variable would be the same lie
    /// `backend.rs` warns about in the preflight code.
    #[test]
    fn an_environment_override_wins_and_disables_the_control() {
        let k = knob("LOGSCOPE_MATVEC_SG");
        let r = resolve(&k, Some(&Value::Flag(false)), |n| (n == "LOGSCOPE_MATVEC_SG").then(|| "1".into()));
        assert_eq!(r.value, Value::Flag(true), "the UI value won over the environment");
        assert_eq!(r.source, Source::Env);
        assert!(!editable(&r), "the control must be disabled while the environment owns it");
        assert_eq!(r.raw.as_deref(), Some("1"));

        // And applying the panel's settings must not write over it.
        let apply = env_to_apply(&[("LOGSCOPE_MATVEC_SG", Value::Flag(false))], |n| {
            (n == "LOGSCOPE_MATVEC_SG").then(|| "1".to_string())
        });
        assert!(apply.is_empty(), "the panel overwrote a shell override: {apply:?}");
    }

    /// A mistyped variable currently looks exactly like an unset one. It should not.
    #[test]
    fn an_unparsable_environment_value_is_reported_rather_than_obeyed() {
        let k = knob("LOGSCOPE_PREFILL_CHUNK");
        let r = resolve(&k, None, |n| (n == "LOGSCOPE_PREFILL_CHUNK").then(|| "lots".into()));
        assert_eq!(r.source, Source::EnvUnparsed);
        assert_eq!(r.value, k.default, "an unreadable value must not become the setting");
        assert_eq!(r.raw.as_deref(), Some("lots"));
        assert!(editable(&r), "nothing usable owns the knob, so the operator may still set it");
        // The flag knobs take words as well as digits, since a shell user writes both.
        let f = knob("LOGSCOPE_MATVEC_SG");
        assert_eq!(f.parse("on"), Some(Value::Flag(true)));
        assert_eq!(f.parse("FALSE"), Some(Value::Flag(false)));
        assert_eq!(f.parse("maybe"), None);
    }

    /// The old spelling is still honoured — `PREFILL_CHUNK` without the prefix is named in
    /// `backend.rs` as a silent-ignore that cost real test cycles.
    #[test]
    fn the_legacy_alias_is_still_read() {
        let k = knob("LOGSCOPE_PREFILL_CHUNK");
        let r = resolve(&k, None, |n| (n == "PREFILL_CHUNK").then(|| "512".into()));
        assert_eq!(r.value, Value::Count(512));
        assert_eq!(r.source, Source::Env);
        assert!(knob("LOGSCOPE_MATVEC_SG").alias.is_none());
    }

    /// "Within reason": these are dispatch sizes on a watchdog-bounded GPU. A prefill chunk
    /// of 100,000 does not make a slow run, it makes a device-lost.
    #[test]
    fn counts_are_clamped_to_a_range_the_hardware_survives() {
        let k = knob("LOGSCOPE_PREFILL_CHUNK");
        assert_eq!(k.clamp(Value::Count(100_000)), Value::Count(k.range.1));
        assert_eq!(k.clamp(Value::Count(0)), Value::Count(k.range.0));
        assert_eq!(k.clamp(Value::Count(512)), Value::Count(512));
        // and the clamp applies to what arrives from the environment too
        assert_eq!(k.parse("100000"), Some(Value::Count(k.range.1)));
        // a flag written into a count knob (an older settings file) falls back to the default
        assert_eq!(k.clamp(Value::Flag(true)), k.default);
        for k in all() {
            if let Value::Count(n) = k.default {
                assert!(k.range.0 <= n && n <= k.range.1, "{}'s default is outside its own range", k.env);
            }
        }
    }

    /// An untouched panel must change nothing. A knob at its default writes no variable, so
    /// loading a model from a fresh install is byte-identical to loading it before this
    /// existed.
    #[test]
    fn defaults_write_no_environment_at_all() {
        let ui: Vec<(&'static str, Value)> = all().iter().map(|k| (k.env, k.default.clone())).collect();
        assert!(env_to_apply(&ui, none).is_empty());
        // one change, one variable
        let ui = vec![("LOGSCOPE_FLUSH_EVERY", Value::Count(4))];
        assert_eq!(env_to_apply(&ui, none), vec![("LOGSCOPE_FLUSH_EVERY", "4".to_string())]);
    }

    /// The fragile ones are marked, because "this is the one that hangs Asahi" is
    /// information the operator has no other way to get.
    #[test]
    fn the_knobs_that_disable_a_mitigation_are_marked_fragile() {
        let fragile: Vec<&str> = all().iter().filter(|k| k.risk == Risk::Fragile).map(|k| k.env).collect();
        assert!(fragile.contains(&"LOGSCOPE_MATVEC_SG"), "{fragile:?}");
        assert!(fragile.contains(&"LOGSCOPE_WINDOW_ONE_SUBMIT"), "{fragile:?}");
        // every knob explains itself, and says it needs a reload
        for k in all() {
            assert!(k.help.len() > 40, "{} has no real help text", k.env);
            assert_eq!(k.applies, Applies::OnLoad);
        }
    }

    #[test]
    fn the_header_says_what_is_off_default_and_who_owns_it() {
        let ks = all();
        let plain: Vec<(&Knob, Resolved)> =
            ks.iter().map(|k| (k, resolve(k, None, none))).collect();
        assert_eq!(summary(&plain), "all default");

        let mixed: Vec<(&Knob, Resolved)> = ks
            .iter()
            .map(|k| {
                let r = match k.env {
                    "LOGSCOPE_FLUSH_EVERY" => resolve(k, Some(&Value::Count(4)), none),
                    "LOGSCOPE_MATVEC_SG" => resolve(k, None, |n| (n == "LOGSCOPE_MATVEC_SG").then(|| "1".into())),
                    "LOGSCOPE_PREFILL_CHUNK" => {
                        resolve(k, None, |n| (n == "LOGSCOPE_PREFILL_CHUNK").then(|| "???".into()))
                    }
                    _ => resolve(k, None, none),
                };
                (k, r)
            })
            .collect();
        let s = summary(&mixed);
        assert!(s.contains("2 changed"), "{s}");
        assert!(s.contains("1 from the environment"), "{s}");
        assert!(s.contains("1 unreadable"), "{s}");
    }
}
