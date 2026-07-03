//! snapclone-menu — the dynamic submenu *model*.
//!
//! SnapClone's contact cards (and, now, group cards and mailbox rows) show a
//! collapsible submenu of actions. WHICH actions appear, in what order, with
//! which icons, and which are destructive is a policy decision that today lives
//! as hardcoded `if`s scattered in the Qt code (ContactsPage::buildCard). As
//! groups, group mailboxes, and pending/blocked states are added, that
//! branching multiplies and drifts between surfaces.
//!
//! This crate centralizes that decision as pure data→list logic: given an
//! [`Entity`] (its kind + state flags), [`actions_for`] returns the ordered
//! [`MenuAction`]s the UI should render. The Qt side becomes a thin loop —
//! `for a in actions_for(entity) { card.addAction(a.label, a.icon, a.danger, a.id) }`
//! — and dispatches on the returned [`ActionId`]. No widgets here, so the
//! policy is unit-testable and identical across every UI surface and backend.
//!
//! # The four surfaces
//!
//!   * **Contact** card — edit, share location, copy id, remove; gated by
//!     whether the contact is editable / removable / a share target / pending.
//!   * **Group** card — open, rename, manage members, leave; gated by whether
//!     the local user is the group admin.
//!   * **Group mailbox** row and **Contact mailbox** row — the
//!     connection-request inbox: accept / reject / block a pending request, or
//!     open / mute an active conversation.
//!
//! # Edit-as-submenu
//!
//! "Edit contact" can either launch a dialog (legacy) or expand an *inline*
//! sub-panel. The model expresses this with [`ActionKind`]: `Dialog` actions
//! open a separate surface; `Expand` actions tell the card to reveal a nested
//! panel (the inline editor). The UI decides how to render each, but the model
//! says which kind each action is, so the behavior is consistent.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod ffi;

/// What kind of entity a submenu is being built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Contact,
    Group,
    /// A row in the connection-request / conversation inbox.
    MailboxEntry,
}

/// State flags that gate which actions appear. Not every flag is meaningful for
/// every kind; irrelevant flags are simply ignored by that kind's rules.
#[derive(Debug, Clone, Copy, Default)]
pub struct EntityState {
    /// The entry is a pending request (not yet accepted) — inbox semantics.
    pub pending: bool,
    /// The local user can edit this entity (own contacts; groups they admin).
    pub editable: bool,
    /// The entity can be removed/left/deleted by the local user.
    pub removable: bool,
    /// A location-share target (a real peer, not a group or a pending stub).
    pub shareable: bool,
    /// The local user is the admin/owner (groups).
    pub is_admin: bool,
    /// The entry is currently muted (conversations).
    pub muted: bool,
    /// The entity is blocked.
    pub blocked: bool,
}

/// A stable identifier for an action, so the UI can dispatch on it regardless of
/// the (localized) label. Kept exhaustive across all surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionId {
    // Contact
    EditContact,
    ShareLocation,
    CopyId,
    RemoveContact,
    // Group
    OpenGroup,
    RenameGroup,
    ManageMembers,
    LeaveGroup,
    CreateGroup,
    // Mailbox / inbox
    AcceptRequest,
    RejectRequest,
    BlockSender,
    UnblockSender,
    OpenConversation,
    MuteConversation,
    UnmuteConversation,
}

impl ActionId {
    /// A stable string tag (for the C FFI + logging). NOT localized — the UI
    /// maps this to a translated label, or uses `default_label`.
    pub fn tag(self) -> &'static str {
        match self {
            ActionId::EditContact => "edit_contact",
            ActionId::ShareLocation => "share_location",
            ActionId::CopyId => "copy_id",
            ActionId::RemoveContact => "remove_contact",
            ActionId::OpenGroup => "open_group",
            ActionId::RenameGroup => "rename_group",
            ActionId::ManageMembers => "manage_members",
            ActionId::LeaveGroup => "leave_group",
            ActionId::CreateGroup => "create_group",
            ActionId::AcceptRequest => "accept_request",
            ActionId::RejectRequest => "reject_request",
            ActionId::BlockSender => "block_sender",
            ActionId::UnblockSender => "unblock_sender",
            ActionId::OpenConversation => "open_conversation",
            ActionId::MuteConversation => "mute_conversation",
            ActionId::UnmuteConversation => "unmute_conversation",
        }
    }

    /// A reasonable English default label (the Qt side may override via tr()).
    pub fn default_label(self) -> &'static str {
        match self {
            ActionId::EditContact => "Edit contact",
            ActionId::ShareLocation => "Share my location",
            ActionId::CopyId => "Copy ID",
            ActionId::RemoveContact => "Remove contact",
            ActionId::OpenGroup => "Open group",
            ActionId::RenameGroup => "Rename group",
            ActionId::ManageMembers => "Manage members",
            ActionId::LeaveGroup => "Leave group",
            ActionId::CreateGroup => "New group",
            ActionId::AcceptRequest => "Accept",
            ActionId::RejectRequest => "Reject",
            ActionId::BlockSender => "Block",
            ActionId::UnblockSender => "Unblock",
            ActionId::OpenConversation => "Open",
            ActionId::MuteConversation => "Mute",
            ActionId::UnmuteConversation => "Unmute",
        }
    }

    /// The icon name the UI's SvgIcon set uses (matches existing names where
    /// possible: edit/beacon/export/import).
    pub fn icon(self) -> &'static str {
        match self {
            ActionId::EditContact => "edit",
            ActionId::ShareLocation => "beacon",
            ActionId::CopyId => "export",
            ActionId::RemoveContact => "import",
            ActionId::OpenGroup | ActionId::OpenConversation => "open",
            ActionId::RenameGroup => "edit",
            ActionId::ManageMembers => "people",
            ActionId::LeaveGroup => "import",
            ActionId::CreateGroup => "plus",
            ActionId::AcceptRequest => "check",
            ActionId::RejectRequest => "close",
            ActionId::BlockSender | ActionId::UnblockSender => "shield",
            ActionId::MuteConversation | ActionId::UnmuteConversation => "bell",
        }
    }
}

/// How the UI should surface an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// A normal one-shot action (does the thing, closes the submenu).
    Immediate,
    /// Opens a separate surface/dialog.
    Dialog,
    /// Expands an inline nested panel within the card (e.g. inline edit).
    Expand,
}

/// A single rendered action descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuAction {
    pub id: ActionId,
    pub kind: ActionKind,
    /// Destructive (rendered in the danger color, may confirm).
    pub danger: bool,
}

impl MenuAction {
    fn new(id: ActionId, kind: ActionKind, danger: bool) -> Self {
        MenuAction { id, kind, danger }
    }
}

/// The entity a submenu is built for.
#[derive(Debug, Clone, Copy)]
pub struct Entity {
    pub kind: EntityKind,
    pub state: EntityState,
}

impl Entity {
    pub fn contact(state: EntityState) -> Self {
        Entity { kind: EntityKind::Contact, state }
    }
    pub fn group(state: EntityState) -> Self {
        Entity { kind: EntityKind::Group, state }
    }
    pub fn mailbox(state: EntityState) -> Self {
        Entity { kind: EntityKind::MailboxEntry, state }
    }
}

/// The core of the crate: the ordered actions for an entity. Order is the
/// display order (top to bottom); destructive actions come last.
///
/// `edit_inline`: if true, an editable contact's Edit action is `Expand`
/// (inline panel); if false it's `Dialog` (legacy editor). The UI passes its
/// preference so both styles share one ruleset.
pub fn actions_for(entity: &Entity, edit_inline: bool) -> Vec<MenuAction> {
    let s = &entity.state;
    let mut out = Vec::new();
    match entity.kind {
        EntityKind::Contact => {
            if s.editable {
                let kind = if edit_inline { ActionKind::Expand } else { ActionKind::Dialog };
                out.push(MenuAction::new(ActionId::EditContact, kind, false));
            }
            if s.shareable {
                out.push(MenuAction::new(ActionId::ShareLocation, ActionKind::Immediate, false));
            }
            out.push(MenuAction::new(ActionId::CopyId, ActionKind::Immediate, false));
            if s.blocked {
                out.push(MenuAction::new(ActionId::UnblockSender, ActionKind::Immediate, false));
            }
            if s.removable {
                out.push(MenuAction::new(ActionId::RemoveContact, ActionKind::Immediate, true));
            }
        }
        EntityKind::Group => {
            out.push(MenuAction::new(ActionId::OpenGroup, ActionKind::Immediate, false));
            if s.is_admin {
                out.push(MenuAction::new(ActionId::RenameGroup,
                    if edit_inline { ActionKind::Expand } else { ActionKind::Dialog }, false));
                out.push(MenuAction::new(ActionId::ManageMembers, ActionKind::Dialog, false));
            }
            if s.muted {
                out.push(MenuAction::new(ActionId::UnmuteConversation, ActionKind::Immediate, false));
            } else {
                out.push(MenuAction::new(ActionId::MuteConversation, ActionKind::Immediate, false));
            }
            if s.removable {
                // Leaving a group is destructive (you lose the shared history).
                out.push(MenuAction::new(ActionId::LeaveGroup, ActionKind::Immediate, true));
            }
        }
        EntityKind::MailboxEntry => {
            if s.pending {
                // A pending connection request: accept / reject / block.
                out.push(MenuAction::new(ActionId::AcceptRequest, ActionKind::Immediate, false));
                out.push(MenuAction::new(ActionId::RejectRequest, ActionKind::Immediate, false));
                out.push(MenuAction::new(ActionId::BlockSender, ActionKind::Immediate, true));
            } else {
                // An active conversation row.
                out.push(MenuAction::new(ActionId::OpenConversation, ActionKind::Immediate, false));
                if s.muted {
                    out.push(MenuAction::new(ActionId::UnmuteConversation, ActionKind::Immediate, false));
                } else {
                    out.push(MenuAction::new(ActionId::MuteConversation, ActionKind::Immediate, false));
                }
                if s.removable {
                    out.push(MenuAction::new(ActionId::RemoveContact, ActionKind::Immediate, true));
                }
            }
        }
    }
    out
}

/// The top-level actions for a surface header (not tied to one entity) — e.g.
/// the "+" on the groups page that starts group creation.
pub fn header_actions(kind: EntityKind) -> Vec<MenuAction> {
    match kind {
        EntityKind::Group => vec![
            MenuAction::new(ActionId::CreateGroup, ActionKind::Dialog, false),
        ],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[MenuAction]) -> Vec<ActionId> {
        v.iter().map(|a| a.id).collect()
    }

    #[test]
    fn editable_removable_contact_matches_legacy_order() {
        // Mirrors the current ContactsPage hardcoded order: Edit, Share, Copy,
        // Remove — so swapping the inline code for the model is behavior-
        // preserving for the common case.
        let e = Entity::contact(EntityState {
            editable: true, removable: true, shareable: true, ..Default::default()
        });
        assert_eq!(ids(&actions_for(&e, false)), vec![
            ActionId::EditContact, ActionId::ShareLocation,
            ActionId::CopyId, ActionId::RemoveContact,
        ]);
    }

    #[test]
    fn non_shareable_non_removable_contact_drops_those() {
        let e = Entity::contact(EntityState {
            editable: true, removable: false, shareable: false, ..Default::default()
        });
        assert_eq!(ids(&actions_for(&e, false)), vec![
            ActionId::EditContact, ActionId::CopyId,
        ]);
    }

    #[test]
    fn edit_inline_flag_switches_edit_kind() {
        let e = Entity::contact(EntityState { editable: true, ..Default::default() });
        assert_eq!(actions_for(&e, false)[0].kind, ActionKind::Dialog);
        assert_eq!(actions_for(&e, true)[0].kind, ActionKind::Expand);
    }

    #[test]
    fn remove_is_the_only_danger_action_for_contact() {
        let e = Entity::contact(EntityState {
            editable: true, removable: true, shareable: true, ..Default::default()
        });
        let dangers: Vec<_> = actions_for(&e, false)
            .into_iter().filter(|a| a.danger).map(|a| a.id).collect();
        assert_eq!(dangers, vec![ActionId::RemoveContact]);
    }

    #[test]
    fn blocked_contact_offers_unblock_not_block() {
        let e = Entity::contact(EntityState {
            editable: false, removable: true, blocked: true, ..Default::default()
        });
        let got = ids(&actions_for(&e, false));
        assert!(got.contains(&ActionId::UnblockSender));
        assert!(!got.contains(&ActionId::BlockSender));
    }

    #[test]
    fn admin_group_shows_management_actions() {
        let e = Entity::group(EntityState {
            is_admin: true, removable: true, ..Default::default()
        });
        let got = ids(&actions_for(&e, false));
        assert_eq!(got, vec![
            ActionId::OpenGroup, ActionId::RenameGroup, ActionId::ManageMembers,
            ActionId::MuteConversation, ActionId::LeaveGroup,
        ]);
        // Leave is the destructive one.
        assert!(actions_for(&e, false).iter()
            .find(|a| a.id == ActionId::LeaveGroup).unwrap().danger);
    }

    #[test]
    fn member_group_hides_admin_only_actions() {
        let e = Entity::group(EntityState {
            is_admin: false, removable: true, ..Default::default()
        });
        let got = ids(&actions_for(&e, false));
        assert!(!got.contains(&ActionId::RenameGroup));
        assert!(!got.contains(&ActionId::ManageMembers));
        assert!(got.contains(&ActionId::OpenGroup));
        assert!(got.contains(&ActionId::LeaveGroup));
    }

    #[test]
    fn pending_mailbox_entry_is_accept_reject_block() {
        let e = Entity::mailbox(EntityState { pending: true, ..Default::default() });
        assert_eq!(ids(&actions_for(&e, false)), vec![
            ActionId::AcceptRequest, ActionId::RejectRequest, ActionId::BlockSender,
        ]);
    }

    #[test]
    fn active_mailbox_entry_is_open_mute_remove() {
        let e = Entity::mailbox(EntityState {
            pending: false, removable: true, muted: false, ..Default::default()
        });
        assert_eq!(ids(&actions_for(&e, false)), vec![
            ActionId::OpenConversation, ActionId::MuteConversation, ActionId::RemoveContact,
        ]);
    }

    #[test]
    fn muted_conversation_offers_unmute() {
        let e = Entity::mailbox(EntityState {
            pending: false, removable: false, muted: true, ..Default::default()
        });
        let got = ids(&actions_for(&e, false));
        assert!(got.contains(&ActionId::UnmuteConversation));
        assert!(!got.contains(&ActionId::MuteConversation));
    }

    #[test]
    fn group_header_offers_create() {
        assert_eq!(
            header_actions(EntityKind::Group).iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![ActionId::CreateGroup]
        );
        assert!(header_actions(EntityKind::Contact).is_empty());
    }

    #[test]
    fn tags_are_unique_and_stable() {
        // The FFI dispatches on tags; they must be distinct.
        let all = [
            ActionId::EditContact, ActionId::ShareLocation, ActionId::CopyId,
            ActionId::RemoveContact, ActionId::OpenGroup, ActionId::RenameGroup,
            ActionId::ManageMembers, ActionId::LeaveGroup, ActionId::CreateGroup,
            ActionId::AcceptRequest, ActionId::RejectRequest, ActionId::BlockSender,
            ActionId::UnblockSender, ActionId::OpenConversation,
            ActionId::MuteConversation, ActionId::UnmuteConversation,
        ];
        let mut tags: Vec<&str> = all.iter().map(|a| a.tag()).collect();
        let n = tags.len();
        tags.sort();
        tags.dedup();
        assert_eq!(tags.len(), n, "every ActionId tag is unique");
    }
}
