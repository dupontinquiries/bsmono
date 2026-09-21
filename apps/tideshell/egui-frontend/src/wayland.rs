//! Real windows: `systemcompkit::wayland_io::ToplevelSession` in, zcosmic
//! toplevel-management requests out. Everything protocol-shaped stays in
//! systemcompkit; this file only translates between its types and the
//! engine's, and the translation is pure (tested below without a socket).

use systemcompkit::toplevel::{ToplevelInfo, ToplevelRequest};
use systemcompkit::wayland_io::{ConnectError, ToplevelSession};
use tideshell_core::{Action, WindowCard};

use crate::effects::{Executor, Outcome};

pub fn to_card(t: &ToplevelInfo) -> WindowCard {
    WindowCard {
        id: t.id,
        // Untitled windows still need a label on a card.
        title: if t.title.is_empty() { t.app_id.clone() } else { t.title.clone() },
        app_id: t.app_id.clone(),
        activated: t.state.activated,
        maximized: t.state.maximized,
        width: t.geometry.map(|g| g.2),
    }
}

/// The window-management actions, as protocol requests. `None` for actions
/// this transport does not own.
pub fn request_for(a: &Action) -> Option<(u64, ToplevelRequest)> {
    match a {
        Action::Activate(id) => Some((*id, ToplevelRequest::Activate)),
        Action::Close(id) => Some((*id, ToplevelRequest::Close)),
        Action::SetMaximized(id, on) => Some((*id, ToplevelRequest::SetMaximized(*on))),
        _ => None,
    }
}

pub struct Windows {
    session: ToplevelSession,
    last: Vec<WindowCard>,
    /// Our own toplevel (the simulator window) must not appear as a card.
    pub exclude_app_id: Option<String>,
}

impl Windows {
    pub fn connect() -> Result<Windows, ConnectError> {
        Ok(Windows { session: ToplevelSession::connect()?, last: Vec::new(), exclude_app_id: None })
    }

    /// Read the socket and return the window list if it changed.
    pub fn poll(&mut self) -> Option<Vec<WindowCard>> {
        let excl = self.exclude_app_id.clone();
        let cards: Vec<WindowCard> = self
            .session
            .poll()
            .list()
            .iter()
            .filter(|t| excl.as_deref() != Some(t.app_id.as_str()))
            .map(to_card)
            .collect();
        (cards != self.last).then(|| {
            self.last = cards.clone();
            cards
        })
    }

    pub fn capabilities(&self) -> &systemcompkit::toplevel::ManagerCaps {
        self.session.capabilities()
    }
}

/// Window actions go to the compositor; everything else to `fallback`.
pub struct Live<'a> {
    pub windows: &'a mut Windows,
    pub fallback: &'a mut dyn Executor,
}

impl Executor for Live<'_> {
    fn run(&mut self, a: &Action) -> Outcome {
        match request_for(a) {
            Some((id, req)) => match self.windows.session.request(id, req) {
                Ok(()) => Outcome::Done(format!("sent {req:?} to {id}")),
                Err(e) => Outcome::Failed(e.to_string()),
            },
            None => self.fallback.run(a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use systemcompkit::toplevel::ToplevelState;

    #[test]
    fn window_actions_map_to_requests_and_others_do_not() {
        assert_eq!(request_for(&Action::Activate(4)), Some((4, ToplevelRequest::Activate)));
        assert_eq!(request_for(&Action::Close(4)), Some((4, ToplevelRequest::Close)));
        assert_eq!(request_for(&Action::SetMaximized(4, false)), Some((4, ToplevelRequest::SetMaximized(false))));
        assert_eq!(request_for(&Action::SummonLauncher), None);
        assert_eq!(request_for(&Action::ShowKeyboard(true)), None);
    }

    #[test]
    fn card_uses_app_id_when_untitled_and_carries_state() {
        let t = ToplevelInfo {
            id: 7,
            title: String::new(),
            app_id: "cosmic-term".into(),
            state: ToplevelState { activated: true, maximized: true, ..Default::default() },
            geometry: None,
        };
        let c = to_card(&t);
        assert_eq!((c.id, c.title.as_str(), c.activated, c.maximized), (7, "cosmic-term", true, true));
    }
}
