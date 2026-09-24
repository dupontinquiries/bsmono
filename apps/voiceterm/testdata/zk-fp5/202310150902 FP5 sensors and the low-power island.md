# FP5 sensors and the low-power island

Up: [[202310150900 Fairphone 5]] · See also [[202310150907 FP5 mainline Linux gaps]]

The repair document lists these under *Sensors & Security*:

| Sensor | Listed | Visible to mainline Linux |
|---|---|---|
| Fingerprint (in power button) | yes | no |
| Facial recognition | yes | n/a (software) |
| Magnetometer | yes | no |
| Accelerometer | yes | no |
| Gyroscope | yes | no |
| Compass | yes | no |
| Hall switch | yes | no |
| Light sensor | yes | no |
| Proximity sensor | yes | no |

Every one of them is physically present, and none of them appears on any I2C
bus the application processor can address. The component table explains why:
the QCM6490 has a **low-power island (LPI)** containing a DSP and an AI
accelerator. The sensors are wired to the island's own I2C controllers, so
only the sensor DSP can read them; the AP reaches them over QMI, not over a
bus of its own.

```niftyplot
%kind: graph
%title: Where the FP5 sensors actually live
AP as rect : "Application processor (Linux)"
IIO as rect : "IIO subsystem"
PMIC as cylinder : "PMIC ADCs — temps, voltages"
LPI as hexagon : "Low-power island (SLPI/ADSP)"
IMU as circle : "Accel, gyro, magnetometer"
ALS as circle : "Light, proximity"
QMI as diamond : "QMI over fastrpc"
AP → IIO
IIO → PMIC
LPI → IMU
LPI → ALS
AP -- "the only route" → QMI
QMI → LPI
```

The practical consequence: compiling an IMU driver into the kernel finds
nothing, because there is no bus for it to probe. The route is a userspace
client speaking QMI to the sensor service on the island.

The transport for that route exists and is healthy: `CONFIG_QRTR=y`,
`QCOM_QMI_HELPERS=y`, a live `IPCRTR` glink edge on the ADSP, and
`pd-mapper`, `rmtfs`, `tqftpserv` and `qmi-proxy` all running. What is
missing is a client and the knowledge of what the ADSP publishes;
`qrtr-lookup` answers the second. Note the contrast with the cameras, whose
sensors sit on CCI buses the AP owns and therefore work —
[[202310150909 FP5 camera pipeline]].

#sensors #qcm6490 #open-question
