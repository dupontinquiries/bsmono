# Serve over a USB cable

A phone (or any device that can be a USB *gadget*) presents itself as a
USB network adapter when plugged in. depot is already listening on that
interface; this sets up the addresses and the firewall so "plug in to
serve" is literally that.

## Which end can be the gadget

Only a device whose USB controller has device mode can be the adapter:
phones, PinePhone/PineTab, Raspberry Pi Zero/4/5, most SBCs. Laptops and
desktops are host-only — two of them on one cable get nothing (use a
USB-Ethernet dongle on each and a plain cable instead). So the two
working shapes are **phone = depot, plugged into a client**, and **phone
= client, plugged into the depot host**.

```sh
ls /sys/class/udc/            # non-empty: this machine has a USB device controller
```

You should see a controller name on a phone/SBC and nothing on a laptop.

## postmarketOS: already wired

pmOS brings the gadget up at boot: the phone is `172.16.42.1` on `usb0`
and hands the host `172.16.42.2`. Plug in and check both ends:

```niftyplot
%kind: graph
%title: "The USB link"
plane phone {
    P: "depot serve — 172.16.42.1:8000"
}
plane host {
    H: "client — 172.16.42.2"
}
P ↔ H
```

```sh
# on the phone
ip -4 addr show usb0
# on the host
ip -4 addr | grep 172.16.42
```

You should see `172.16.42.1/24` on the phone and `172.16.42.2` on the
host. If the host got nothing, the gadget function is the usual cause:
NCM works with Linux, macOS and Windows 10+; RNDIS only with Windows.
`deviceinfo_usb_network_function` in the pmOS device package selects it.

## Tell depot its USB address
%check: public_url_usb

There is no DNS on a two-node link, so the address is the name. Put it
in the config so generated client files and `bootstrap.sh` use it:

```sh
sed -i 's|^public_url = .*|public_url = "http://172.16.42.1:8000"|' ~/.local/share/depot/depot.toml
# USB-only serving (not over wifi as well):
sed -i 's|^bind = .*|bind = "172.16.42.1:8000"|' ~/.local/share/depot/depot.toml
depot serve            # or: rc-service depot restart, systemctl restart depot
```

You should see `depot serving … on http://172.16.42.1:8000/`. Leave
`bind` at `0.0.0.0:8000` if the phone should also serve over wifi.

## Open the host's firewall for the link

On a Fedora host the new interface lands in the default zone and 8000
is closed. A cable to a device you are holding can be trusted outright:

```sh
sudo firewall-cmd --permanent --zone=trusted --add-interface=usb0
sudo firewall-cmd --reload
curl -sI http://172.16.42.1:8000/api/status | head -1
```

You should see `200 OK`. (Interface naming may give you `enp0s20f0u1`
or similar instead of `usb0`; `ip -4 addr` shows which one has the
`172.16.42.2` address.)

## Configure the client from the USB address

Same as any client, with the USB address in the URL:

```sh
curl -fsSL "http://172.16.42.1:8000/bootstrap.sh?crates=1&toolchain=1" -o bootstrap.sh
DRY=1 sh bootstrap.sh && sudo sh bootstrap.sh
```

You should see files written with `http://172.16.42.1:8000` inside. A USB 2
gadget link moves 30–40 MB/s; USB 3 several times that — a Fedora package
set is minutes, and the many-small-requests pattern of dnf is faster over
the cable than over wifi.

## A gadget on a device that is not pmOS

Two pieces: the kernel gadget (configfs) and an address. The gadget:

```sh
sudo modprobe libcomposite
cd /sys/kernel/config/usb_gadget && sudo mkdir depot && cd depot
echo 0x1d6b | sudo tee idVendor; echo 0x0104 | sudo tee idProduct
sudo mkdir -p strings/0x409 configs/c.1 functions/ncm.usb0
echo depot | sudo tee strings/0x409/product
sudo ln -s functions/ncm.usb0 configs/c.1/
ls /sys/class/udc | sudo tee UDC                    # bind: usb0 appears
```

The address, with systemd-networkd:

```ini
# /etc/systemd/network/usb0.network
[Match]
Name=usb0
[Network]
Address=172.16.42.1/24
DHCPServer=yes
```

You should see `usb0` in `ip link` after binding, and a host that plugs
in gets an address. Persisting the configfs steps at boot is a small
oneshot unit; pmOS's `postmarketos-base` does exactly this.

## When the link does not come up

```sh
dmesg | tail -20                 # on both ends: "NCM" / "RNDIS" / "cdc_ncm" lines, or errors
lsusb                            # on the host: does the phone show up as a network device?
ip link                          # is the interface there but down?
```

You should see the gadget enumerate on the host within a second of
plugging in. A cable that charges but never enumerates is a power-only
cable — more common than any software fault.
