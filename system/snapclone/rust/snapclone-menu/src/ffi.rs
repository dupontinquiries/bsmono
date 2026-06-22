//! C FFI for snapclone-menu.
//!
//! The Qt side calls `sc_menu_actions_for(...)` with the entity kind + state
//! flags and a caller-provided array; the function fills it with descriptors
//! (action tag, label, icon, kind, danger) and returns the count. The C++ then
//! loops and calls ContactCard::addAction for each, dispatching on the tag.
//! No allocation crosses the boundary — the caller owns the buffer; strings are
//! 'static and returned as const char* (no freeing needed).

use crate::{actions_for, header_actions, ActionKind, Entity, EntityKind, EntityState};
use std::os::raw::c_char;
use std::panic::catch_unwind;

/// C-compatible entity kind.
pub const SC_MENU_KIND_CONTACT: i32 = 0;
pub const SC_MENU_KIND_GROUP: i32 = 1;
pub const SC_MENU_KIND_MAILBOX: i32 = 2;

/// C-compatible action kind.
pub const SC_MENU_ACTION_IMMEDIATE: i32 = 0;
pub const SC_MENU_ACTION_DIALOG: i32 = 1;
pub const SC_MENU_ACTION_EXPAND: i32 = 2;

/// State flags as a bitfield (so one C int carries the whole EntityState).
pub const SC_MENU_STATE_PENDING: u32 = 1 << 0;
pub const SC_MENU_STATE_EDITABLE: u32 = 1 << 1;
pub const SC_MENU_STATE_REMOVABLE: u32 = 1 << 2;
pub const SC_MENU_STATE_SHAREABLE: u32 = 1 << 3;
pub const SC_MENU_STATE_IS_ADMIN: u32 = 1 << 4;
pub const SC_MENU_STATE_MUTED: u32 = 1 << 5;
pub const SC_MENU_STATE_BLOCKED: u32 = 1 << 6;

/// One action descriptor handed back to C. All strings are 'static (point into
/// the Rust binary's read-only data) — the caller must NOT free them and should
/// copy them if it needs them past the program's life (it won't).
#[repr(C)]
pub struct ScMenuAction {
    /// Stable ASCII tag for dispatch (e.g. "edit_contact"). NUL-terminated.
    pub tag: *const c_char,
    /// Default English label (UI may ignore and translate from tag).
    pub label: *const c_char,
    /// Icon name for the SvgIcon set.
    pub icon: *const c_char,
    /// SC_MENU_ACTION_* kind.
    pub kind: i32,
    /// 1 if destructive, else 0.
    pub danger: i32,
}

fn kind_from_c(k: i32) -> Option<EntityKind> {
    match k {
        SC_MENU_KIND_CONTACT => Some(EntityKind::Contact),
        SC_MENU_KIND_GROUP => Some(EntityKind::Group),
        SC_MENU_KIND_MAILBOX => Some(EntityKind::MailboxEntry),
        _ => None,
    }
}

fn state_from_bits(bits: u32) -> EntityState {
    EntityState {
        pending: bits & SC_MENU_STATE_PENDING != 0,
        editable: bits & SC_MENU_STATE_EDITABLE != 0,
        removable: bits & SC_MENU_STATE_REMOVABLE != 0,
        shareable: bits & SC_MENU_STATE_SHAREABLE != 0,
        is_admin: bits & SC_MENU_STATE_IS_ADMIN != 0,
        muted: bits & SC_MENU_STATE_MUTED != 0,
        blocked: bits & SC_MENU_STATE_BLOCKED != 0,
    }
}

fn kind_to_c(k: ActionKind) -> i32 {
    match k {
        ActionKind::Immediate => SC_MENU_ACTION_IMMEDIATE,
        ActionKind::Dialog => SC_MENU_ACTION_DIALOG,
        ActionKind::Expand => SC_MENU_ACTION_EXPAND,
    }
}

// Each ActionId's three strings as NUL-terminated 'static byte arrays. We build
// them at call time from the model's &'static str (which are NOT NUL-terminated
// Rust string literals) — but C needs NUL termination. Since the set is fixed
// and small, we use a lookup of pre-NUL-terminated literals here.
fn c_strings(id: crate::ActionId) -> (*const c_char, *const c_char, *const c_char) {
    macro_rules! cstr {
        ($s:literal) => {
            concat!($s, "\0").as_ptr() as *const c_char
        };
    }
    use crate::ActionId::*;
    match id {
        EditContact => (cstr!("edit_contact"), cstr!("Edit contact"), cstr!("edit")),
        ShareLocation => (cstr!("share_location"), cstr!("Share my location"), cstr!("beacon")),
        CopyId => (cstr!("copy_id"), cstr!("Copy ID"), cstr!("export")),
        RemoveContact => (cstr!("remove_contact"), cstr!("Remove contact"), cstr!("import")),
        OpenGroup => (cstr!("open_group"), cstr!("Open group"), cstr!("open")),
        RenameGroup => (cstr!("rename_group"), cstr!("Rename group"), cstr!("edit")),
        ManageMembers => (cstr!("manage_members"), cstr!("Manage members"), cstr!("people")),
        LeaveGroup => (cstr!("leave_group"), cstr!("Leave group"), cstr!("import")),
        CreateGroup => (cstr!("create_group"), cstr!("New group"), cstr!("plus")),
        AcceptRequest => (cstr!("accept_request"), cstr!("Accept"), cstr!("check")),
        RejectRequest => (cstr!("reject_request"), cstr!("Reject"), cstr!("close")),
        BlockSender => (cstr!("block_sender"), cstr!("Block"), cstr!("shield")),
        UnblockSender => (cstr!("unblock_sender"), cstr!("Unblock"), cstr!("shield")),
        OpenConversation => (cstr!("open_conversation"), cstr!("Open"), cstr!("open")),
        MuteConversation => (cstr!("mute_conversation"), cstr!("Mute"), cstr!("bell")),
        UnmuteConversation => (cstr!("unmute_conversation"), cstr!("Unmute"), cstr!("bell")),
    }
}

fn fill(out: *mut ScMenuAction, cap: i32, actions: Vec<crate::MenuAction>) -> i32 {
    if out.is_null() || cap <= 0 {
        return 0;
    }
    let n = (actions.len() as i32).min(cap);
    for (i, a) in actions.iter().take(n as usize).enumerate() {
        let (tag, label, icon) = c_strings(a.id);
        // SAFETY: out has room for `cap` entries; i < n <= cap.
        unsafe {
            let slot = out.add(i);
            (*slot).tag = tag;
            (*slot).label = label;
            (*slot).icon = icon;
            (*slot).kind = kind_to_c(a.kind);
            (*slot).danger = i32::from(a.danger);
        }
    }
    n
}

/// Fill `out` (capacity `cap`) with the actions for the given entity. Returns
/// the number written (<= cap). `edit_inline` != 0 makes the Edit/Rename action
/// an Expand (inline panel) rather than a Dialog.
///
/// # Safety
/// `out` must point to at least `cap` `ScMenuAction`s, or be null (returns 0).
#[no_mangle]
pub unsafe extern "C" fn sc_menu_actions_for(
    kind: i32,
    state_bits: u32,
    edit_inline: i32,
    out: *mut ScMenuAction,
    cap: i32,
) -> i32 {
    let res = catch_unwind(|| {
        let k = match kind_from_c(kind) {
            Some(k) => k,
            None => return 0,
        };
        let entity = Entity { kind: k, state: state_from_bits(state_bits) };
        fill(out, cap, actions_for(&entity, edit_inline != 0))
    });
    res.unwrap_or(0)
}

/// Fill `out` with the header (surface-level) actions for a kind — e.g. the
/// group page's "New group". Returns count written.
///
/// # Safety
/// Same contract as `sc_menu_actions_for`.
#[no_mangle]
pub unsafe extern "C" fn sc_menu_header_actions(
    kind: i32,
    out: *mut ScMenuAction,
    cap: i32,
) -> i32 {
    let res = catch_unwind(|| {
        let k = match kind_from_c(kind) {
            Some(k) => k,
            None => return 0,
        };
        fill(out, cap, header_actions(k))
    });
    res.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn tag_of(a: &ScMenuAction) -> &str {
        unsafe { CStr::from_ptr(a.tag).to_str().unwrap() }
    }

    fn blank() -> ScMenuAction {
        ScMenuAction {
            tag: std::ptr::null(), label: std::ptr::null(),
            icon: std::ptr::null(), kind: 0, danger: 0,
        }
    }

    #[test]
    fn ffi_contact_fills_expected() {
        let mut buf = [blank(), blank(), blank(), blank(), blank(), blank()];
        let bits = SC_MENU_STATE_EDITABLE | SC_MENU_STATE_REMOVABLE | SC_MENU_STATE_SHAREABLE;
        let n = unsafe {
            sc_menu_actions_for(SC_MENU_KIND_CONTACT, bits, 0, buf.as_mut_ptr(), 6)
        };
        assert_eq!(n, 4);
        assert_eq!(tag_of(&buf[0]), "edit_contact");
        assert_eq!(buf[0].kind, SC_MENU_ACTION_DIALOG);
        assert_eq!(tag_of(&buf[3]), "remove_contact");
        assert_eq!(buf[3].danger, 1);
    }

    #[test]
    fn ffi_edit_inline_makes_expand() {
        let mut buf = [blank(), blank()];
        let n = unsafe {
            sc_menu_actions_for(SC_MENU_KIND_CONTACT, SC_MENU_STATE_EDITABLE, 1,
                                buf.as_mut_ptr(), 2)
        };
        assert_eq!(n, 2); // edit + copy
        assert_eq!(tag_of(&buf[0]), "edit_contact");
        assert_eq!(buf[0].kind, SC_MENU_ACTION_EXPAND);
    }

    #[test]
    fn ffi_respects_capacity() {
        let mut buf = [blank(), blank()];
        let bits = SC_MENU_STATE_EDITABLE | SC_MENU_STATE_REMOVABLE | SC_MENU_STATE_SHAREABLE;
        // 4 actions but cap 2 → only 2 written, no overflow.
        let n = unsafe {
            sc_menu_actions_for(SC_MENU_KIND_CONTACT, bits, 0, buf.as_mut_ptr(), 2)
        };
        assert_eq!(n, 2);
    }

    #[test]
    fn ffi_group_header_create() {
        let mut buf = [blank()];
        let n = unsafe { sc_menu_header_actions(SC_MENU_KIND_GROUP, buf.as_mut_ptr(), 1) };
        assert_eq!(n, 1);
        assert_eq!(tag_of(&buf[0]), "create_group");
    }

    #[test]
    fn ffi_null_and_bad_kind_safe() {
        assert_eq!(unsafe { sc_menu_actions_for(SC_MENU_KIND_CONTACT, 0, 0, std::ptr::null_mut(), 4) }, 0);
        let mut buf = [blank()];
        assert_eq!(unsafe { sc_menu_actions_for(999, 0, 0, buf.as_mut_ptr(), 1) }, 0);
    }
}
