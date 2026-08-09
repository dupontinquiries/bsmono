//! remote_browse — pick the deploy destination instead of typing it blind.
//!
//! The push page has fields for user, host and target triple, and none for
//! **where on the device the code should land**. The GTK build browses it:
//! `main.rs` calls `deploy::list_remote_dirs` and shows a folder picker. The
//! egui port uses five of `cockpit::deploy`'s functions and not that one, so
//! the destination was unreachable.
//!
//! ## The navigation is a pure model, and the SSH is injected
//!
//! `Browser` holds the path stack and the listing; the caller supplies a
//! function that actually talks to the device. That is not ceremony — it is the
//! only way this gets tested at all, since the alternative is a test that needs
//! a machine to SSH into. The real caller passes
//! `cockpit::deploy::list_remote_dirs`.
//!
//! ## Paths come back resolved, and that matters
//!
//! `list_remote_dirs` returns the *resolved* `pwd`, not the string that was
//! asked for. So entering `~` yields `/home/user` and the browser stores what
//! the device says rather than what the user typed — otherwise a deploy target
//! recorded as `~/proj` means different directories to `ssh` and to `rsync`
//! depending on who runs them.

/// One directory listing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    /// Absolute path as the device resolved it.
    pub path: String,
    /// Subdirectory names, no `.` or `..`.
    pub dirs: Vec<String>,
}

/// What the browser is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    Loading,
    Ready(Listing),
    /// The device said no. Kept as a state rather than a popup so the path
    /// stack survives — a permission error two levels deep should not throw
    /// away the navigation that got there.
    Failed(String),
}

impl Default for State {
    fn default() -> Self { State::Idle }
}

#[derive(Debug, Clone, Default)]
pub struct Browser {
    pub user: String,
    pub host: String,
    pub state: State,
    /// Resolved paths visited, oldest first. Enables "up" without re-deriving
    /// a parent by string surgery, which is how `/` becomes `` and a browser
    /// ends up listing nothing.
    stack: Vec<String>,
    /// The path the user has settled on.
    pub chosen: Option<String>,
}

impl Browser {
    pub fn new(user: &str, host: &str) -> Browser {
        Browser { user: user.into(), host: host.into(), ..Default::default() }
    }

    pub fn current(&self) -> Option<&str> {
        match &self.state {
            State::Ready(l) => Some(l.path.as_str()),
            _ => self.stack.last().map(|s| s.as_str()),
        }
    }

    pub fn can_go_up(&self) -> bool { self.stack.len() > 1 }

    /// Ask the device for a listing.
    ///
    /// `lister` is `(user, host, path) -> Result<(resolved, dirs), err>`, which
    /// is `cockpit::deploy::list_remote_dirs`' shape exactly, so the real one
    /// drops in without an adapter.
    pub fn open<F>(&mut self, path: &str, lister: F)
    where F: Fn(&str, &str, &str) -> Result<(String, Vec<String>), String> {
        // Refuse before the network round trip rather than after: an empty host
        // produces an ssh invocation that hangs on DNS, and "nothing happened
        // for thirty seconds" is a worse error than a sentence.
        if self.host.trim().is_empty() {
            self.state = State::Failed("no host selected".into());
            return;
        }
        self.state = State::Loading;
        match lister(&self.user, &self.host, path) {
            Ok((resolved, dirs)) => {
                // Store what the DEVICE says, not what was asked for.
                if self.stack.last().map(|s| s.as_str()) != Some(resolved.as_str()) {
                    self.stack.push(resolved.clone());
                }
                self.state = State::Ready(Listing { path: resolved, dirs });
            }
            Err(e) => self.state = State::Failed(e),
        }
    }

    /// Descend into a subdirectory of the current listing.
    pub fn enter<F>(&mut self, name: &str, lister: F)
    where F: Fn(&str, &str, &str) -> Result<(String, Vec<String>), String> {
        let Some(base) = self.current().map(|s| s.to_string()) else { return };
        let joined = if base.ends_with('/') {
            format!("{base}{name}")
        } else {
            format!("{base}/{name}")
        };
        self.open(&joined, lister);
    }

    /// Go back to the previously visited directory.
    ///
    /// Pops the stack rather than trimming the current path: the device may
    /// have resolved a symlink, so the textual parent of where we are is not
    /// necessarily where we came from.
    pub fn up<F>(&mut self, lister: F)
    where F: Fn(&str, &str, &str) -> Result<(String, Vec<String>), String> {
        if self.stack.len() < 2 { return; }
        self.stack.pop();
        let prev = self.stack.pop().unwrap_or_default();
        self.open(&prev, lister);
    }

    /// Settle on the current directory as the deploy destination.
    pub fn choose(&mut self) -> Option<&str> {
        if let State::Ready(l) = &self.state {
            self.chosen = Some(l.path.clone());
        }
        self.chosen.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A fake device. Maps the path asked for to (resolved, subdirs) — the
    /// resolution step is modelled because it is where the real one surprises
    /// you: `~` comes back as an absolute path.
    fn device() -> impl Fn(&str, &str, &str) -> Result<(String, Vec<String>), String> {
        let mut fs: HashMap<&str, (&str, Vec<&str>)> = HashMap::new();
        fs.insert("~", ("/home/pi", vec!["proj", "tmp"]));
        fs.insert("/home/pi", ("/home/pi", vec!["proj", "tmp"]));
        fs.insert("/home/pi/proj", ("/home/pi/proj", vec!["src", "target"]));
        fs.insert("/home/pi/proj/src", ("/home/pi/proj/src", vec![]));
        fs.insert("/home/pi/secret", ("", vec![]));
        move |_u, _h, p| match fs.get(p) {
            Some(("", _)) => Err("Permission denied".into()),
            Some((res, dirs)) => {
                Ok((res.to_string(), dirs.iter().map(|s| s.to_string()).collect()))
            }
            None => Err("no such directory".into()),
        }
    }

    #[test]
    fn a_tilde_resolves_to_what_the_device_says() {
        // Storing "~" would mean different directories to ssh and rsync
        // depending on who runs them.
        let mut b = Browser::new("pi", "10.0.0.5");
        b.open("~", device());
        assert_eq!(b.current(), Some("/home/pi"));
        assert!(matches!(&b.state, State::Ready(l) if l.dirs == vec!["proj", "tmp"]));
    }

    #[test]
    fn entering_a_subdirectory_joins_without_doubling_the_slash() {
        let mut b = Browser::new("pi", "10.0.0.5");
        b.open("~", device());
        b.enter("proj", device());
        assert_eq!(b.current(), Some("/home/pi/proj"));
    }

    #[test]
    fn going_up_pops_the_stack_rather_than_trimming_the_path() {
        // The device may resolve a symlink, so the textual parent of where we
        // are is not necessarily where we came from.
        let mut b = Browser::new("pi", "10.0.0.5");
        b.open("~", device());
        b.enter("proj", device());
        b.enter("src", device());
        assert_eq!(b.current(), Some("/home/pi/proj/src"));
        b.up(device());
        assert_eq!(b.current(), Some("/home/pi/proj"));
        b.up(device());
        assert_eq!(b.current(), Some("/home/pi"));
    }

    #[test]
    fn up_at_the_root_of_the_visit_does_nothing() {
        let mut b = Browser::new("pi", "10.0.0.5");
        b.open("~", device());
        assert!(!b.can_go_up());
        b.up(device());
        assert_eq!(b.current(), Some("/home/pi"), "still where we were");
    }

    #[test]
    fn a_failure_keeps_the_navigation_that_reached_it() {
        // A permission error two levels deep should not throw away the path
        // stack — the user's next move is "go back", and that needs the stack.
        let mut b = Browser::new("pi", "10.0.0.5");
        b.open("~", device());
        b.enter("proj", device());
        b.open("/home/pi/secret", device());
        assert!(matches!(b.state, State::Failed(ref e) if e.contains("Permission")));
        assert!(b.can_go_up(), "the stack survived");
        b.up(device());
        assert_eq!(b.current(), Some("/home/pi"));
    }

    #[test]
    fn an_empty_host_fails_before_the_network_round_trip() {
        // An empty host produces an ssh that hangs on DNS, and "nothing
        // happened for thirty seconds" is a worse error than a sentence.
        let mut b = Browser::new("pi", "  ");
        b.open("~", |_u, _h, _p| panic!("must not reach the device"));
        assert!(matches!(b.state, State::Failed(ref e) if e.contains("no host")));
    }

    #[test]
    fn choosing_records_the_resolved_path() {
        let mut b = Browser::new("pi", "10.0.0.5");
        b.open("~", device());
        b.enter("proj", device());
        assert_eq!(b.choose(), Some("/home/pi/proj"));
        assert_eq!(b.chosen.as_deref(), Some("/home/pi/proj"));
    }

    #[test]
    fn choosing_before_a_listing_arrives_records_nothing() {
        // Otherwise a deploy fires at whatever string happened to be in the
        // field while the first listing was still in flight.
        let mut b = Browser::new("pi", "10.0.0.5");
        assert_eq!(b.choose(), None);
        b.state = State::Loading;
        assert_eq!(b.choose(), None);
    }

    #[test]
    fn revisiting_the_same_directory_does_not_grow_the_stack() {
        // Refreshing is an ordinary action and must not make "up" walk back
        // through repeats of the same folder.
        let mut b = Browser::new("pi", "10.0.0.5");
        b.open("~", device());
        b.open("/home/pi", device());
        b.open("/home/pi", device());
        assert!(!b.can_go_up());
    }
}
