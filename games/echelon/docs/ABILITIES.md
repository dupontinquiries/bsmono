# Echelon Ability Atlas

Abilities are **data**: a trigger kind (`Auto` fires itself, `Active` goes
through `Cmd::Cast`) plus an ordered op list from a fixed vocabulary —
paperbound's sigil pattern generalized. The vocabulary (echelon-data
`Op`): Heal, ShieldGive, EnergyGive, SelfDamage, KillSelf, AreaDamage,
ChainDamage, Dot, Stun, SpeedMul, AtkSpeedMul, DmgTakenMul, RegenHp,
Cloak, Blink, Summon. All ability state (stuns, buffs, dots, cloaks,
cooldowns, timed life) is folded into the lockstep hash. Cloak interacts
with `detector: true` defs (Processor, BilePit, Oculus): cloaked units
vanish from enemy auto-acquisition unless a detector has them in sight.

Everything below cites the ability actually present in the mod's data.

## Genetron
| unit | ability (source) | mapping |
|---|---|---|
| ACR | MULERepair | Auto ally-Mechanical Heal 5, r2.5 |
| Fixer | FixerRepair | Auto ally-Mechanical Heal 8, r4 |
| Fixer, Blaze | Speed Cloak — "Cloaks, +40% speed" | Active self: SpeedMul 1.4x 8s + Cloak 8s |
| Blitzer | Overdrive | Active self: SpeedMul 1.7x 6s |
| Aquila | Nanite Grenade — "Stuns … in an area" | Active point e50: Stun r1.8 3s |
| Inciter | ELOR Protocol — "Relocates the Inciter" | Active point: Blink 8 |
| Processor | System Offload — "+15% attack speed in area" | Active point e50: AtkSpeedMul area (also `detector`) |
| Jupiter | Revenant Bomb — "+dmg vs Armored" | Active point: AreaDamage 60 (+40 Armored) r1.2 |
| Boar | Ignition Blast | Active self: SpeedMul 1.5x (contact air dmg: planned) |
| RelayTower | System Recharge — "Transfers energy" | Auto ally: EnergyGive 50 |
| Virtus | Selffix Protocol | Auto self <50% hp: RegenHp 10/s 5s |
| Tesla | Launch Shock Charge | Active point e50: AreaDamage 25 + Stun 1s (Cloak Charge: planned) |

## Xayid
| unit | ability (source) | mapping |
|---|---|---|
| Mendling | Mend — "restores life to biological" | Auto ally-Bio e30: Heal 45 |
| Mendling | Sacrificial Mutation | Active ally: Heal 200 + KillSelf |
| Mendling, Casnolisk | Covering Spray — "Cloaks a friendly unit" | Active ally e25: Cloak 12s |
| Casnolisk | Explosive Glob — "60 dmg in area, knockback" | Active point e75: AreaDamage 60 (knockback: planned) |
| Casnolisk | Stagnating Bile — "stunning units" | Active point e75: Stun r1.2 2s |
| Exterminator | Incendiary Infusion — periodic damage | Active enemy: Dot 12/s 8s |
| Prowler | Blinding Burst — "disables vision, attacks" | Active self: Stun r1.6 2s (blind≈stun) |
| Prowler | Place Acid Nest — burrowed trap | Active point: Summon AcidNest, 90s life |
| Xayidling | Dash | Active self: SpeedMul 1.6x 4s |
| Xayithoan | Adaptive Injection — "+40% AS friendly" | Active ally e50: AtkSpeedMul 1.4x (enemy branch: planned) |
| Xayithoan | Protective Cloud — "-30% damage taken" | Active point e75: DmgTakenMul 0.7x area |
| Kraken | Volatile Discharge — delayed blast | Active point: AreaDamage 80 r1.4 (delay: planned) |
| Massalisk | Hunker | Active self: DmgTakenMul 0.6x + SpeedMul 0.4x |

## Keiron
| unit | ability (source) | mapping |
|---|---|---|
| Pariah | Projection — "move through units, faster" | Active self: SpeedMul 1.5x (phasing: planned) |
| Pulsar | Dampener — "-x% spell/splash damage" | Auto self <60% hp: DmgTakenMul 0.75x |
| Echo | Sonic Image — teleport-back beacon | Active point: Blink 7 (image mechanic: simplified) |
| Meridian | Lightning Bolt — "bounces onto targets" | Active enemy e75: ChainDamage 40 x3 |
| Meridian | Convergence — "+dmg vs Mechanical, area" | Active point e100: AreaDamage 50 (+30 Mech) |
| Umbra | Stealth Compulsion — "re-cloaks at life cost" | Active self: SelfDamage 40 + Cloak 10s |
| Gyre | Shackle — "disables movement of air units" | Active point e75: Stun r1.4 3s |
| Paragon | Emissary Orb — "damage in an area" | Active enemy: AreaDamage 60 r0.9 |
| Aurora | Emanator — vision ward, Detector | Active point e50: Summon Oculus, 30s life |
| Fuse | (shield support role) | Auto ally e10: ShieldGive 30 |

## Not yet in the engine (tracked)
Burrow pairs (Repulsor/Interdictor/AcidNest concealment), transports
(Courier, Ferrion, Aperture load/unload), Gyre Annex capture, Myriad's
Elytra hangar, Harbinger Judgement mode, Aegis Relocate, Roamer Scour
(needs the vision system), Echo's true image-return, Tesla cloak-field
charge, knockbacks, and Boar's contact damage. Each lands as either a new
Op or a new trigger kind — the data files won't change shape.
