//! usbkb -- a USB keyboard made of this machine.
//!
//! Three small parts, none of which needs root except what the printed scripts do:
//!
//! * [`hid`]: characters to 8-byte boot-keyboard reports (US layout), pure and tested.
//! * [`gadget`]: the configfs script that creates the keyboard gadget, the one that removes it, and the
//!   data-role swap the USB-C port needs first. They are only PRINTED: the person at the keyboard pastes
//!   them into a root shell. Nothing here runs `sudo`.
//! * [`status`]: a read-only survey of the USB-C ports, the UDC and `/dev/hidg*`, to see what the
//!   hardware will do before anything is changed.

pub mod gadget;
pub mod hid;
pub mod status;
