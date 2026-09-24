# FP5 mainline Linux gaps

Up: [[202310150900 Fairphone 5]] · Cause: [[202310150902 FP5 sensors and the low-power island]]

What the phone can do under stock Android versus what postmarketOS reaches
today. This note is the one that dates fastest — treat every row as a claim
with an owner, not a fact.

| Capability | Stock | Mainline | Blocker |
|---|---|---|---|
| Display, touch, 90 Hz | yes | yes | — |
| GPU (Adreno 643) | yes | yes, Turnip/freedreno | — |
| Modem, Wi-Fi, BT | yes | yes | — |
| Accel / gyro / magnetometer | yes | no | sensors behind the LPI, no QMI client |
| Light / proximity | yes | no | same |
| Fingerprint | yes | no | reader behind the power button, no driver |
| Cameras — all three enumerate | yes | yes, via libcamera | — |
| Hardware ISP | yes | no | libcamera falls back to the `simple` pipeline handler and the software ISP; CAMSS is loaded but not used for processing |
| Sensor crop / pixel array | yes | no | the three sensor drivers implement no `V4L2_SEL_TGT_CROP`/`CROP_BOUNDS`/`NATIVE_SIZE`, so libcamera defaults them and says "the sensor kernel driver needs to be fixed" |
| Sensor tuning data | yes | no | `imx800`, `imx858`, `s5kjn1` are absent from libcamera's sensor-properties database and `CameraSensorHelper`, so gains and delays are guessed |
| Autofocus, ultra-wide | yes | yes | — |
| Autofocus, main camera | yes | no | no lens actuator bound on `cci@ac4a000/i2c-bus@0`; only the ultra-wide has one |
| Flash / torch | yes | no | `CONFIG_V4L2_FLASH_LED_CLASS` is not set |
| Backlight smoothness | yes | partial | DPU write bug, worked around in software |
| Double-tap to wake | yes | no | touchscreen driver exposes no gesture knob |

```niftyplot
%kind: graph
%title: Blockers by kind
Boot as hexagon : "Works"
Fw as hexagon : "Needs firmware protocol"
Drv as hexagon : "Needs a driver or config"
GPU as rect : "GPU"
Modem as rect : "Modem, Wi-Fi"
Sens as circle : "Sensors"
FP as circle : "Fingerprint"
Cam as circle : "Hardware ISP"
DTW as circle : "Double-tap wake"
AF as circle : "Autofocus (ultra-wide)"
Bl as diamond : "Backlight ramp"
GPU → Boot
Modem → Boot
Sens → Fw
FP → Fw
Cam → Drv
AF → Boot
DTW → Drv
Bl → Drv
```

Measured on kernel 7.2.2, 2026-09-23. `imx800`, `imx858` and `s5kjn1` all
bind their drivers and all three appear in `cam -l`. The `dw9800k` node at
`17-000e` binds the mainline `dw9719` driver and registers as a Lens subdev
beside `imx858`; sweeping `focus_absolute` 0 → 1023 on `/dev/v4l-subdev29`
moves the lens audibly and the value reads back exactly. See
[[202310150909 FP5 camera pipeline]].

The split matters for planning: the *Needs a driver or config* column is
ordinary kernel work, while the *Needs firmware protocol* column means
writing a userspace client for a proprietary interface, which is a different
size of project entirely.

#postmarketos #roadmap #open-question
