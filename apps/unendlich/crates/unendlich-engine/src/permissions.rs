//! Per-player permissions — the gate between "what the player asks
//! to do" and "what they're actually allowed to do."
//!
//! ## Why this exists
//!
//! Single-player today implicitly grants every action: every break,
//! every place, every flight toggle just happens. Multiplayer
//! servers need fine-grained control: creative players can fly,
//! survival players can't; admins can run /tp, regular players
//! can't; spectators can't break blocks at all.
//!
//! The cheap migration path is to gate every action through a
//! `Permissions` check NOW, in single-player, with all-true
//! defaults. When the server arrives, only the source of the
//! `Permissions` value changes (server pushes it, client honors
//! it). Action sites stay identical.
//!
//! ## What this is NOT
//!
//! This is NOT survival mechanics (hunger, health, stamina). Those
//! are STATE that mutates over time and feeds back into permission
//! checks (`can_sprint = base.can_sprint && hunger > LOW_HUNGER`),
//! but they're a separate concern. They live in their own module
//! when they exist.
//!
//! Permissions are MOSTLY-STATIC ACL grants set by an authority
//! (server admin / game mode / world settings) and CHECKED by
//! action sites. Survival state is the dynamic feedback that
//! interacts with them.

/// Permission level for command access. Cumulative: a Moderator
/// can do everything a Player can, an Admin can do everything a
/// Moderator can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionLevel {
    /// No commands allowed.
    None,
    /// Basic player commands: /seed, /spawn, /tell, etc.
    Player,
    /// World-affecting commands: /tp self, /weather, /time.
    Moderator,
    /// Full server control: /tp others, /op, /ban, /gamemode.
    Admin,
}

/// Per-player permission grants. Single-player default is all-true
/// (`Permissions::full_admin`). Multiplayer servers push restricted
/// values to clients; clients honor them client-side AND the server
/// validates server-side (defense in depth).
#[derive(Debug, Clone, Copy)]
pub struct Permissions {
    /// Can break blocks via raycast / left-click.
    pub can_break_block: bool,
    /// Can place blocks via raycast / right-click.
    pub can_place_block: bool,
    /// Can toggle between fly mode and physics mode.
    pub can_toggle_flight: bool,
    /// Can open the inventory UI (when one exists).
    pub can_open_inventory: bool,
    /// Can light fires / spawn heat sources (F9).
    pub can_light_fire: bool,
    /// Can use F4-F8 debug overlays.
    pub can_use_debug_overlay: bool,
    /// Maximum command level this player is allowed to invoke.
    pub command_level: PermissionLevel,
}

impl Permissions {
    /// All-true / full-admin defaults. Single-player flow uses this.
    pub fn full_admin() -> Self {
        Self {
            can_break_block: true,
            can_place_block: true,
            can_toggle_flight: true,
            can_open_inventory: true,
            can_light_fire: true,
            can_use_debug_overlay: true,
            command_level: PermissionLevel::Admin,
        }
    }

    /// Survival player: can break/place, can use inventory, can NOT
    /// fly, no admin commands. Used as the multiplayer default for
    /// regular connections.
    pub fn survival_player() -> Self {
        Self {
            can_break_block: true,
            can_place_block: true,
            can_toggle_flight: false,
            can_open_inventory: true,
            can_light_fire: true,
            can_use_debug_overlay: false,
            command_level: PermissionLevel::Player,
        }
    }

    /// Spectator: read-only. Can fly, can NOT break/place/light.
    pub fn spectator() -> Self {
        Self {
            can_break_block: false,
            can_place_block: false,
            can_toggle_flight: true,
            can_open_inventory: false,
            can_light_fire: false,
            can_use_debug_overlay: true,
            command_level: PermissionLevel::None,
        }
    }

    /// Whether this permission set allows running a command at the
    /// given level.
    pub fn can_use_command(&self, required: PermissionLevel) -> bool {
        self.command_level >= required
    }
}

impl Default for Permissions {
    /// Default is `full_admin` — single-player has no restrictions.
    /// Servers should explicitly set restricted values.
    fn default() -> Self { Self::full_admin() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_admin_grants_everything() {
        let p = Permissions::full_admin();
        assert!(p.can_break_block);
        assert!(p.can_place_block);
        assert!(p.can_toggle_flight);
        assert!(p.can_open_inventory);
        assert!(p.can_light_fire);
        assert!(p.can_use_debug_overlay);
        assert!(p.can_use_command(PermissionLevel::Admin));
        assert!(p.can_use_command(PermissionLevel::Player));
    }

    #[test]
    fn survival_blocks_flight() {
        let p = Permissions::survival_player();
        assert!(p.can_break_block);
        assert!(!p.can_toggle_flight);
        assert!(!p.can_use_command(PermissionLevel::Admin));
        assert!(p.can_use_command(PermissionLevel::Player));
    }

    #[test]
    fn spectator_is_read_only() {
        let p = Permissions::spectator();
        assert!(!p.can_break_block);
        assert!(!p.can_place_block);
        assert!(p.can_toggle_flight);
        assert!(!p.can_use_command(PermissionLevel::Player));
    }

    /// Permission levels are ordered: Admin > Moderator > Player > None.
    /// This test pins the ordering since command-gating depends on it.
    #[test]
    fn permission_levels_are_ordered() {
        assert!(PermissionLevel::Admin > PermissionLevel::Moderator);
        assert!(PermissionLevel::Moderator > PermissionLevel::Player);
        assert!(PermissionLevel::Player > PermissionLevel::None);
    }

    /// `can_use_command` honors the cumulative ordering: an Admin
    /// can run Player-level commands; a Player cannot run Admin-
    /// level commands.
    #[test]
    fn can_use_command_is_cumulative() {
        let admin = Permissions { command_level: PermissionLevel::Admin, ..Permissions::full_admin() };
        let player = Permissions { command_level: PermissionLevel::Player, ..Permissions::full_admin() };
        // Admin gets everything.
        assert!(admin.can_use_command(PermissionLevel::Admin));
        assert!(admin.can_use_command(PermissionLevel::Moderator));
        assert!(admin.can_use_command(PermissionLevel::Player));
        // Player gets only Player and None.
        assert!(!player.can_use_command(PermissionLevel::Admin));
        assert!(!player.can_use_command(PermissionLevel::Moderator));
        assert!(player.can_use_command(PermissionLevel::Player));
        assert!(player.can_use_command(PermissionLevel::None));
    }

    /// Default is all-true — important for single-player code paths
    /// that don't explicitly construct Permissions.
    #[test]
    fn default_is_full_admin() {
        let p = Permissions::default();
        assert!(p.can_break_block);
        assert!(p.can_use_command(PermissionLevel::Admin));
    }
}
