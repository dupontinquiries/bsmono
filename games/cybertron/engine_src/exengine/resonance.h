/* resonance -- Blackstar's resonance field.
 *
 * From the film script: he activates a field and nearby Decepticons "hesitate,
 * recalibrate, begin acting in sync without realizing it." It is not an attack --
 * it is coordination. Units inside the field stop behaving as scattered
 * individuals and start moving as one.
 *
 * In-game: an aware "coordinator" (an apex/elite) projects a field; other enemies
 * inside it are SYNCED -- they share the coordinator's awareness and gain a
 * coordination edge, strongest at the center and fading to nothing at the rim.
 * Kill the coordinator and the field collapses. This module is the pure geometry:
 * given coordinator positions and a point, how strongly is it synced?
 */
#ifndef EX_RESONANCE_H
#define EX_RESONANCE_H

#define EX_RESONANCE_RADIUS 34.0f

/* Sync strength in [0,1] for point `p`: the strongest field it sits in -- 1 at a
   coordinator, ramping linearly to 0 at `radius`, and 0 outside every field (or
   when there are no coordinators). */
float ex_resonance_strength(const float coords[][3], int n, const float p[3], float radius);

#endif /* EX_RESONANCE_H */
