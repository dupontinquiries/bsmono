//! Ask what kind of thing this is, instead of matching words.
//!
//! `choose_path` decides between five routes by string matching, and
//! the tables it matches against hold 1,296 literal words across four
//! modules. Every routing failure reported has been one of those words
//! being wrong:
//!
//!   "a walled yard with a hall and a gate"  -> matched `arch`, built
//!                                             ONE ARCH
//!   "a giant tree with vines"               -> took `vines` as the
//!                                             subject and asked what
//!                                             a vine is made of
//!   "a gorilla statue"                      -> `gorilla` was in no
//!                                             animal list, so it was
//!                                             a biped, and built a man
//!
//! The response each time was to add words. That is not generalising,
//! it is enlarging the table, and the next unlisted noun fails the same
//! way. There is no list of every noun.
//!
//! A model already knows a gorilla is a four-legged animal, that vines
//! are a decoration on a tree, and that a yard with a hall and a gate
//! is three things. So it is asked, once, in one cheap turn, and the
//! answer routes the build. The tables stay as a FALLBACK for when
//! there is no model or the answer is unusable -- which is what a
//! table is good for, and not what it was being used for.
//!
//! WHY THIS IS CHEAP ENOUGH TO ALWAYS DO
//!
//! Six short lines out, one line back. It is the smallest turn in the
//! crate, and it replaces the decision that everything downstream
//! depends on -- a wrong route wastes every turn after it, so this is
//! the cheapest place to be right.

/// What kind of thing was asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// The head noun: what the thing IS, with modifiers stripped.
    pub subject: String,
    pub kind: Kind,
    /// Roughly how tall, in blocks, when the request implies a size.
    pub height: Option<i32>,
    /// Distinct things named, when it is several.
    pub parts: Vec<String>,
    /// Anything the model said that did not fit the form.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A creature or a person, or a statue of one.
    Figure,
    /// A building: it has an inside.
    Building,
    /// Ground: a canyon, a hill, a lake. Made by cutting, mostly.
    Landscape,
    /// A thing with no inside and no anatomy: an obelisk, a chess
    /// piece, a lantern.
    Object,
    /// Several distinct things in one request.
    Several,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.trim().to_lowercase().as_str() {
            "figure" | "creature" | "person" | "animal" | "statue" => Kind::Figure,
            "building" | "structure" => Kind::Building,
            "landscape" | "terrain" | "ground" => Kind::Landscape,
            "object" | "thing" => Kind::Object,
            "several" | "many" | "group" | "multiple" => Kind::Several,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self { Kind::Figure => "figure", Kind::Building => "building",
                     Kind::Landscape => "landscape", Kind::Object => "object",
                     Kind::Several => "several" }
    }
}

/// The routing question. Deliberately the smallest prompt in the
/// crate: a wrong route wastes every turn after it, so it is worth
/// asking, and it is only worth asking if it is cheap.
pub fn prompt(request: &str) -> String {
    format!("\
A player asked for: {request}

Say what KIND of thing this is, so it can be built the right way. Nothing else.

  kind figure|building|landscape|object|several
  subject <the ONE thing it is, with adjectives and extras stripped>
  height <roughly how tall in blocks, or leave out if they did not say>
  parts <only if kind is several: the distinct things, separated by semicolons>

  figure     a creature or a person, or a statue of one -- it has a body
  building   it has an INSIDE you could walk into
  landscape  ground: a canyon, a hill, a lake. Mostly made by cutting away
  object     no inside and no body: an obelisk, a lantern, a chess piece
  several    more than one distinct thing

The SUBJECT is what the thing is, not what is on it. \"A giant tree with vines\" is a
TREE -- the vines are on it. \"A gorilla statue\" is a gorilla. \"A walled yard with a
hall and a gate\" is SEVERAL, and the parts are the yard, the hall and the gate.

EXAMPLES
  a giant tree with vines      -> kind object / subject tree / height 40
  a gorilla statue             -> kind figure / subject gorilla / height 20
  a walled yard with a hall    -> kind several / subject yard / parts yard; hall
  carve a great canyon         -> kind landscape / subject canyon
  a windmill on the hill       -> kind building / subject windmill")
}

/// Read the routing answer. Tolerant, because this turn must never be
/// the reason a build fails: an unusable answer falls back to the
/// tables, which is what they are for.
pub fn parse(reply: &str) -> Option<Reading> {
    let mut kind = None;
    let mut subject = String::new();
    let mut height = None;
    let mut parts = Vec::new();
    let mut notes = Vec::new();
    for line in reply.lines() {
        let t = line.trim().trim_start_matches(['-', '*', '•']).trim();
        if t.is_empty() { continue; }
        // "kind figure" or "kind: figure" or "kind figure / subject x"
        for chunk in t.split('/') {
            let c = chunk.trim().replace(':', " ");
            let mut w = c.split_whitespace();
            match w.next().map(|x| x.to_lowercase()).as_deref() {
                Some("kind") => kind = w.next().and_then(Kind::parse).or(kind),
                Some("subject") | Some("it") => {
                    let s: String = w.collect::<Vec<_>>().join(" ");
                    if !s.is_empty() { subject = s.to_lowercase(); }
                }
                Some("height") | Some("tall") =>
                    height = w.find_map(|x| x.trim_matches(|c: char| !c.is_ascii_digit())
                                        .parse::<i32>().ok()).filter(|h| *h > 0).or(height),
                Some("parts") => parts.extend(w.collect::<Vec<_>>().join(" ")
                    .split(|c| c == ';' || c == ',')
                    .map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty() && p.len() < 60)),
                _ => if !c.trim().is_empty() { notes.push(c.trim().to_string()) },
            }
        }
    }
    // A kind is the whole point; without one there is nothing to route
    // on and the tables are better than a guess.
    let kind = kind?;
    if subject.is_empty() {
        // a `several` with parts still knows what it is about
        subject = parts.first().cloned().unwrap_or_default();
    }
    if subject.is_empty() { return None; }
    // strip an article the model left on
    for a in ["a ", "an ", "the "] { subject = subject.trim_start_matches(a).to_string(); }
    notes.truncate(4);
    parts.truncate(12);
    Some(Reading { subject, kind, height, parts, notes })
}

impl Reading {
    pub fn summary(&self) -> String {
        let mut s = format!("{}: {}", self.kind.name(), self.subject);
        if let Some(h) = self.height { s.push_str(&format!(", about {h} blocks")); }
        if !self.parts.is_empty() { s.push_str(&format!(" [{}]", self.parts.join("; "))); }
        s
    }
    /// Enough to act on? A `several` with one part is not several, and
    /// a figure with no subject is nothing.
    pub fn usable(&self) -> bool {
        !self.subject.is_empty()
            && (self.kind != Kind::Several || self.parts.len() >= 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_reported_failures_are_answerable_in_one_line() {
        // every routing failure reported was one of 1,296 literal words
        // being wrong, and the response each time was to add words
        let cases = [
            ("kind object\nsubject tree\nheight 40\n", Kind::Object, "tree"),
            ("kind figure\nsubject gorilla\nheight 20\n", Kind::Figure, "gorilla"),
            ("kind several\nsubject yard\nparts yard; hall; gate\n", Kind::Several, "yard"),
        ];
        for (reply, kind, subject) in cases {
            let r = parse(reply).expect("a reading");
            assert_eq!(r.kind, kind);
            assert_eq!(r.subject, subject, "the subject is the THING, not what is on it");
            assert!(r.usable());
        }
        // "a giant tree with vines" took `vines` as the subject; the
        // model says tree
        assert_eq!(parse("kind object\nsubject tree\n").unwrap().subject, "tree");
        // and several carries its parts
        let s = parse("kind several\nsubject yard\nparts yard; hall; gate\n").unwrap();
        assert_eq!(s.parts.len(), 3);
        assert!(s.summary().contains("yard; hall; gate"));
    }

    #[test]
    fn the_forms_a_model_actually_writes_all_parse() {
        // one line, slashes, colons, articles, bullets
        for reply in ["kind figure / subject gorilla / height 20",
                      "kind: figure\nsubject: a gorilla\nheight: 20",
                      "- kind figure\n- subject the gorilla\n- height 20 blocks",
                      "KIND FIGURE\nSUBJECT Gorilla\nHEIGHT 20"] {
            let r = parse(reply).unwrap_or_else(|| panic!("{reply:?}"));
            assert_eq!(r.kind, Kind::Figure, "{reply:?}");
            assert_eq!(r.subject, "gorilla", "articles are stripped: {reply:?}");
            assert_eq!(r.height, Some(20), "{reply:?}");
        }
    }

    #[test]
    fn an_unusable_answer_falls_back_rather_than_guessing() {
        // this turn must NEVER be the reason a build fails: the tables
        // are the fallback, which is what a table is good for
        assert!(parse("I think that would be a lovely build!").is_none());
        assert!(parse("").is_none());
        assert!(parse("subject gorilla\n").is_none(), "no kind is nothing to route on");
        // a `several` with one part is not several
        let one = parse("kind several\nsubject hall\nparts hall\n").unwrap();
        assert!(!one.usable(), "one thing is not several");
        // and a kind with no subject anywhere
        assert!(parse("kind building\n").is_none());
    }

    #[test]
    fn the_prompt_is_the_smallest_in_the_crate_and_says_what_subject_means() {
        let p = prompt("a giant tree with vines");
        assert!(p.len() < 1500, "{} chars -- a wrong route wastes every turn after it, \
                 so this must be cheap enough to always ask", p.len());
        assert!(p.contains("not what is on it"), "the subject rule is stated: {p}");
        // the sentence wraps, so compare on whitespace-normalised text
        let flat = p.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("is a TREE -- the vines are on it"), "{flat}");
        assert!(p.contains("it has an INSIDE you could walk into"), "each kind is defined");
        // and its own examples parse
        for line in p.lines().filter(|l| l.contains("->")) {
            let answer = line.split("->").nth(1).unwrap().replace(" / ", "\n");
            let r = parse(&answer);
            assert!(r.is_some(), "{line}");
            assert!(r.unwrap().usable() || answer.contains("several"), "{line}");
        }
    }
}
