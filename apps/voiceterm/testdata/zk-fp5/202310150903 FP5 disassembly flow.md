# FP5 disassembly flow

Up: [[202310150900 Fairphone 5]] · Parts: [[202310150904 FP5 spare parts and screws]]

Every repair starts the same way, which is what makes the phone easy: back
cover and battery come off with no tools at all, and the display needs only a
PH00 driver.

```niftyplot
%kind: graph
%title: FP5 disassembly order
Back as rect : "Back cover (no tools)"
Batt as rect : "Battery (lift the tab)"
Disp as rect : "Display — 8 black screws"
Top as rect : "Top cover — 1 lid + 6 silver screws"
Cam as circle : "Rear cameras"
Selfie as circle : "Selfie camera"
Ear as circle : "Earpiece"
Spk as rect : "Loudspeaker — 5 silver screws"
USB as rect : "USB-C port"
Vib as circle : "Vibration motor"
PCB as hexagon : "PCB and mid-frame"
Water as diamond : "Water ingress check"
Back → Batt
Batt → Disp
Batt → Top
Batt → Spk
Top → Cam
Top → Selfie
Top → Ear
Spk → USB
Spk → Vib
Disp → PCB
Top → PCB
PCB → Water
```

## Cautions before disassembly

1. Power off and remove the battery.
2. Hold the power button more than 5 seconds *after* the battery is out, to
   drain residual charge from the capacitors.
3. ESD protection — the document singles out winter as the risk.
4. Do not mix the two screw types; see [[202310150904 FP5 spare parts and screws]].

#repair #procedure
