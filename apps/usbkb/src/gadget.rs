//! The commands that make the keyboard gadget exist. Printed, never run: they need root, and the person
//! at the keyboard decides when to paste them.

/// The standard HID boot keyboard report descriptor (8-byte input, 1-byte LED output).
pub const REPORT_DESC: [u8; 63] = [
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
    0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x03, 0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01,
    0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x03, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65,
    0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xC0,
];

pub const GADGET: &str = "usbkb";

fn escaped_desc() -> String {
    REPORT_DESC.iter().map(|b| format!("\\x{b:02x}")).collect()
}

/// Make the data-role of USB-C `port` (e.g. `port2`) `device`, so this machine presents itself instead of
/// hosting. The partner (the phone) must then be the host; whether it will swap is the thing to find out.
pub fn swap_script(port: &str) -> String {
    format!(
        "# USB-C data role: this machine becomes the device. Needs root.\n\
         echo device > /sys/class/typec/{port}/data_role\n\
         cat /sys/class/typec/{port}/data_role   # expect: host [device]\n"
    )
}

/// Create the keyboard gadget and bind it to the first UDC.
pub fn setup_script() -> String {
    format!(
        "# Create a USB keyboard gadget. Needs root.\n\
         set -e\n\
         modprobe libcomposite\n\
         modprobe usb_f_hid\n\
         mountpoint -q /sys/kernel/config || mount -t configfs none /sys/kernel/config\n\
         cd /sys/kernel/config/usb_gadget\n\
         mkdir -p {g} && cd {g}\n\
         echo 0x1d6b > idVendor; echo 0x0104 > idProduct; echo 0x0100 > bcdDevice; echo 0x0200 > bcdUSB\n\
         mkdir -p strings/0x409\n\
         echo usbkb > strings/0x409/manufacturer; echo 'usbkb keyboard' > strings/0x409/product; echo 0001 > strings/0x409/serialnumber\n\
         mkdir -p configs/c.1/strings/0x409\n\
         echo keyboard > configs/c.1/strings/0x409/configuration; echo 120 > configs/c.1/MaxPower\n\
         mkdir -p functions/hid.usb0\n\
         echo 1 > functions/hid.usb0/protocol; echo 1 > functions/hid.usb0/subclass; echo 8 > functions/hid.usb0/report_length\n\
         printf '{desc}' > functions/hid.usb0/report_desc\n\
         [ -e configs/c.1/hid.usb0 ] || ln -s functions/hid.usb0 configs/c.1/\n\
         ls /sys/class/udc | head -1 > UDC\n\
         echo \"bound to UDC: $(cat UDC)\"\n\
         # let the person typing use /dev/hidg0 without root:\n\
         sleep 1; chmod 666 /dev/hidg0 && echo '/dev/hidg0 is writable'\n",
        g = GADGET,
        desc = escaped_desc()
    )
}

/// Remove it again.
pub fn teardown_script() -> String {
    format!(
        "# Remove the keyboard gadget. Needs root.\n\
         cd /sys/kernel/config/usb_gadget/{g} 2>/dev/null || exit 0\n\
         echo '' > UDC\n\
         rm -f configs/c.1/hid.usb0\n\
         rmdir configs/c.1/strings/0x409 configs/c.1 functions/hid.usb0 strings/0x409 2>/dev/null\n\
         cd .. && rmdir {g}\n",
        g = GADGET
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_descriptor_is_a_boot_keyboard_and_encodes_to_every_byte() {
        assert_eq!(REPORT_DESC[0..4], [0x05, 0x01, 0x09, 0x06], "generic desktop / keyboard");
        assert_eq!(*REPORT_DESC.last().unwrap(), 0xC0, "end collection");
        let s = escaped_desc();
        assert_eq!(s.matches("\\x").count(), REPORT_DESC.len());
    }

    #[test]
    fn the_setup_script_loads_the_modules_binds_a_udc_and_opens_the_node() {
        let s = setup_script();
        for must in ["modprobe libcomposite", "modprobe usb_f_hid", "report_length", "> UDC", "chmod 666 /dev/hidg0"] {
            assert!(s.contains(must), "setup script has {must:?}");
        }
        assert!(!s.contains("sudo"), "nothing here runs sudo itself");
    }

    #[test]
    fn the_swap_script_names_the_port_it_was_given() {
        assert!(swap_script("port2").contains("/sys/class/typec/port2/data_role"));
    }

    #[test]
    fn teardown_undoes_what_setup_created() {
        let t = teardown_script();
        assert!(t.contains("echo '' > UDC") && t.contains("rmdir"));
    }
}
