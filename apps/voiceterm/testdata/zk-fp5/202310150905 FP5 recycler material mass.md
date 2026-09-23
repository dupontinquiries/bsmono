# FP5 recycler material mass

Up: [[202310150900 Fairphone 5]]

Masses are Fairphone's own estimates and vary with the analytical method.
The percentage column is the share of that material found in the named
component, not the share of the phone.

| Material | CAS | Mass (g) | Mostly in | Share |
|---|---|---|---|---|
| Aluminium | 7429-90-5 | 38.69 | mid-frame | 94.4 % |
| Copper | 7440-50-8 | 16.85 | shields, PCB, battery, foil | 25.6 / 28.4 / 6.1 / 9.0 % |
| Cobalt | 7440-48-4 | 12.32 | battery | 99.8 % |
| Back cover (PC) | — | 12.68 | back cover | 100 % |
| Middle frame (PBT+30%GF) | — | 5.75 | mid-frame | 100 % |
| Magnesium | 7439-95-4 | 4.96 | display frame | 91.0 % |
| Top antenna cover (PC) | — | 5.33 | antenna cover | 100 % |
| Nickel | 7440-02-0 | 3.05 | shields, cell, steel sheet | 41.5 / 16.8 / 19.5 % |
| Display frame (PC+30%GF) | — | 3.06 | display frame | 100 % |
| Battery frame (PC/ABS) | — | 2.32 | battery frame | 100 % |
| Lithium | 7439-93-2 | 1.47 | battery | 98.8 % |
| Zinc | 7440-10-0 | 1.03 | shields | 94 % |
| Tin | 7440-10-0 | 0.87 | solder paste, battery solder | 51 / 25 % |
| Tungsten | 7440-10-0 | 0.75 | vibration mechanism | 99 % |
| Rare earths | various | 0.15 | speaker, vibration, cameras | 100 % |
| Silver | 7440-22-4 | 0.083 | solder, battery package, passives | 22 / 14 / 19 % |
| Gold | 7440-57-5 | 0.023 | cameras, PCB, connectors | 100 % |
| Mica | 12001-26-2 | 0.017 | BTB connector | 100 % |
| Indium | 7440-74-6 | ~0 | display panel | 76.8 % |
| Palladium | 7440-05-03 | ~0 | ICs, passives | 100 % |
| Platinum | 7440-06-04 | ~0 | ICs | 100 % |

Two data-quality traps worth preserving rather than cleaning up, because a
demo vault should contain the kind of mess real documents have:

- Tungsten, tin and zinc are all printed with CAS **7440-10-0**, which is
  rhodium. Three different materials, one wrong number, copied across the
  table.
- Cobalt appears twice, as 12.32 g at 99.8 % and as 12.2971 g at <99 %.

```niftyplot
%kind: graph
%title: Mass by material, heaviest first
Al as rect : "Aluminium 38.7 g"
Cu as rect : "Copper 16.9 g"
Co as rect : "Cobalt 12.3 g"
Back as rect : "Back cover PC 12.7 g"
Mg as rect : "Magnesium 5.0 g"
Li as rect : "Lithium 1.5 g"
Au as circle : "Gold 0.023 g"
Frame as hexagon : "Structure"
Batt as hexagon : "Battery"
Board as hexagon : "Boards and contacts"
Al → Frame
Mg → Frame
Back → Frame
Co → Batt
Li → Batt
Cu → Board
Au → Board
```

#recycling #materials #data-quality
