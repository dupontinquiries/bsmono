# FP5 hardware specification

Up: [[202310150900 Fairphone 5]]

## Core

| Area | Value |
|---|---|
| Chipset | Qualcomm QCM6490, 64-bit octa-core |
| CPU | 1× "Gold+" 2.7 GHz, 3× "Gold" 2.4 GHz, 4× "Silver" 1.9 GHz |
| GPU | Adreno 643 @ 812 MHz (the component table in §6 says 642L — see note below) |
| RAM | 8 GB |
| Storage | 256 GB, micro-SD to 2 TB |
| OS as shipped | Android 13, updates promised to 2031 |

The GPU figure is inconsistent *within the document*: the specification page
says Adreno 643 @ 812 MHz, the component table says Adreno 642L. The device
reports 643 (Turnip, FD643). Worth keeping as a reminder that vendor
documents disagree with themselves.

## Display

| Property | Value |
|---|---|
| Panel | 6.46" Full HD+ OLED, 1224×2700 |
| Refresh | 90 Hz |
| Touch sample rate | 300 Hz |
| Brightness | 800 nits typical, 880 nits peak |
| Contrast | 1,000,000:1 |
| Colour | 1.07 billion, P3 |
| Cover | Gorilla Glass 5, fully laminated |
| Aspect | 20:9 |

## Cameras

| Position | Sensor | Optics | Notes |
|---|---|---|---|
| Main | Sony IMX800, 1/1.49", 1.0 µm | 26 mm eq, F1.88, 6 elements | OIS + EIS, ToF, 8× digital |
| Ultra-wide | Sony IMX858, 1/2.51", 0.7 µm | 13 mm eq, F2.2, 6 elements | macro to 2.5 cm, ToF, EIS |
| Selfie | Samsung JN1 50 MP, 1/2.76", 0.64 µm | 22 mm eq, F2.45 | display used as flash |

All three appear on the I2C camera buses as `imx800`, `imx858` and `s5kjn1`.

## Battery

| Property | Value |
|---|---|
| Capacity | 4200 mAh, removable |
| Nominal / cut-off / charging voltage | 3.87 V / 3.0 V / 4.45 V |
| Internal impedance | < 130 mΩ |
| Charge temperature range | 0–60 °C |
| Discharge temperature range | −20–60 °C |
| Charging | 30 W, 50 % in 20 minutes |
| Rated life | 459 h idle, 38 h calls, 18 h browsing |

Fairphone recommends one full charge/discharge cycle every 3–6 months.

#fairphone #specs
