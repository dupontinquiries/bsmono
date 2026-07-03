/* snapclone_menu_ffi.h — C ABI for the dynamic submenu model.
 *
 * Given an entity (contact / group / mailbox row) and its state flags, returns
 * the ordered action descriptors the UI should render. The Qt side loops the
 * result and calls ContactCard::addAction(...) for each, dispatching on `tag`.
 *
 * The policy (which actions, order, icons, danger, dialog-vs-inline) lives in
 * the tested Rust crate, so every surface and backend renders the same menus.
 * Link libsnapclone_menu.a. Strings returned are 'static — do NOT free them.
 *
 * Example:
 *   ScMenuAction acts[8];
 *   uint32_t flags = SC_MENU_STATE_EDITABLE | SC_MENU_STATE_REMOVABLE
 *                  | SC_MENU_STATE_SHAREABLE;
 *   int n = sc_menu_actions_for(SC_MENU_KIND_CONTACT, flags,
 *                               useInlineEdit ? 1 : 0, acts, 8);
 *   for (int i = 0; i < n; ++i) {
 *       card->addAction(tr(acts[i].label), [=]{ dispatch(acts[i].tag); },
 *                       QString::fromUtf8(acts[i].icon),
 *                       acts[i].danger != 0);
 *   }
 */
#ifndef SNAPCLONE_MENU_FFI_H
#define SNAPCLONE_MENU_FFI_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* entity kinds */
#define SC_MENU_KIND_CONTACT   0
#define SC_MENU_KIND_GROUP     1
#define SC_MENU_KIND_MAILBOX   2

/* action kinds (how the UI should surface it) */
#define SC_MENU_ACTION_IMMEDIATE 0  /* do it, close the submenu */
#define SC_MENU_ACTION_DIALOG    1  /* open a separate surface/dialog */
#define SC_MENU_ACTION_EXPAND    2  /* expand an inline nested panel (inline edit) */

/* state flags (bitfield) */
#define SC_MENU_STATE_PENDING    (1u << 0)
#define SC_MENU_STATE_EDITABLE   (1u << 1)
#define SC_MENU_STATE_REMOVABLE  (1u << 2)
#define SC_MENU_STATE_SHAREABLE  (1u << 3)
#define SC_MENU_STATE_IS_ADMIN   (1u << 4)
#define SC_MENU_STATE_MUTED      (1u << 5)
#define SC_MENU_STATE_BLOCKED    (1u << 6)

/* One action descriptor. tag/label/icon are 'static const char* — never free. */
typedef struct ScMenuAction {
    const char *tag;    /* stable ASCII dispatch id, e.g. "edit_contact" */
    const char *label;  /* default English label (UI may translate from tag) */
    const char *icon;   /* SvgIcon name */
    int32_t     kind;   /* SC_MENU_ACTION_* */
    int32_t     danger; /* 1 = destructive */
} ScMenuAction;

/* Fill `out` (capacity `cap`) with the actions for an entity; returns the count
 * written (<= cap). `edit_inline` != 0 makes Edit/Rename an EXPAND action. */
int32_t sc_menu_actions_for(int32_t kind, uint32_t state_bits, int32_t edit_inline,
                            ScMenuAction *out, int32_t cap);

/* Fill `out` with surface-level header actions (e.g. group "New group"). */
int32_t sc_menu_header_actions(int32_t kind, ScMenuAction *out, int32_t cap);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_MENU_FFI_H */
