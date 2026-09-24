# FP5 camera pipeline

Up: [[202310150900 Fairphone 5]] · Gaps: [[202310150907 FP5 mainline Linux gaps]]

Measured on postmarketOS edge, kernel 7.2.2, 2026-09-23. Everything here was
run on the device; nothing is inferred from the spec sheet.

## What binds

| I2C | Chip | Driver | Role |
|---|---|---|---|
| 16-001a | imx800 | `imx800` | main, `cci@ac4a000/i2c-bus@0` |
| 17-0029 | imx858 | `imx858` | ultra-wide, `cci@ac4a000/i2c-bus@1` |
| 19-0010 | s5kjn1 | `s5kjn1` | selfie, `cci@ac4b000/i2c-bus@1` |
| 17-000e | dw9800k | **`dw9719`** | lens actuator, ultra-wide |

`CONFIG_VIDEO_QCOM_CAMSS=m`, `VIDEO_IMX800=m`, `VIDEO_IMX858=m`,
`VIDEO_S5KJN1=m`, `VIDEO_DW9719=m`. `V4L2_FLASH_LED_CLASS` is **not** set, so
there is no torch or flash control. `/dev/media0` is `qcom-camss` on
`platform:acb3000.isp`, with 31 subdevs.

## Autofocus works, on one camera

The `dw9800k` device tree node binds the mainline `dw9719` driver — the part
is compatible enough that no new driver is needed, which is why this went
unnoticed. It registers as entity 423, a **Lens** subdev with no pads, beside
`imx858` (entity 421).

```sh
S=/dev/v4l-subdev29   # focus_absolute 0x009a090a, min 0 max 1023 step 1
for v in 0 256 512 768 1023 512 0; do
  sudo v4l2-ctl -d $S --set-ctrl=focus_absolute=$v; sleep 0.7
done
```

The lens is audible across the travel and every value reads back exactly.
That is the whole of snapclone's `VcmController` dependency, and it needs no
preview and no working ISP.

The main camera has **no lens entity on the graph**. Its bus carries only the
sensor and an EEPROM, so there is nothing to drive even though the phone
focuses that camera under Android. Whether the actuator is undeclared in the
device tree or wired behind the camera DSP is unresolved.

## Three cameras, but the software ISP

`cam -l` lists all three. The line that matters is what handles them:

```
INFO Camera camera_manager.cpp:223 Adding camera '…/camera@1a' for pipeline handler simple
```

`simple`, not a CAMSS pipeline — libcamera is debayering on the CPU while the
hardware ISP sits loaded and unused. Frames work; the power and thermal cost
is the price.

## Three named defects, each its own patch

1. **Sensor selection ioctls.** All three drivers fail
   `V4L2_SEL_TGT_CROP`, `CROP_BOUNDS` and `NATIVE_SIZE`, so libcamera
   defaults `PixelArraySize` and `PixelArrayActiveAreas` and logs *the sensor
   kernel driver needs to be fixed*. Kernel side, contained, and libcamera
   names the document to follow.
2. **Sensor properties.** None of `imx800`, `imx858`, `s5kjn1` is in
   libcamera's sensor-properties database or its `CameraSensorHelper`, so
   gain conversion and sensor delays are unverified guesses. libcamera side,
   mostly data.
3. **CAMSS pipeline.** Moving off `simple` onto the hardware ISP. The
   largest of the three by a wide margin.

## A format that looks wrong

On their CSI pads, `imx858` and `s5kjn1` both report
`UYVY8_1X16/1920x1080` while `imx800` reports `SBGGR10_1X10/4096x3072`. A
50 MP Bayer sensor advertising packed YUV at 1080p reads as a driver default
rather than the hardware, and it would cap both at 1080p whatever the
pipeline does. Worth checking before anyone optimises around it.

#camera #postmarketos #measured
