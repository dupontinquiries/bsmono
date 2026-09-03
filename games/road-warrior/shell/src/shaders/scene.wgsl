// Side-view scene shader: procedural ground/sky, a car silhouette
// (body/cabin/wheels, shaped and colored per CarKind), a boss with a
// telegraph flash, and a grit/dust/grain post pass — composited in one
// fullscreen fragment shader.
//
// Confirmed working: run through the real naga/wgpu pipeline (Vulkan via
// lavapipe under Xvfb) repeatedly this session, screenshotted at every step.
// Two real bugs were caught this way and are fixed: a sky/ground vertical
// flip from misjudging wgpu's Y-up NDC convention, and a uniform-buffer
// size mismatch from using vec3<f32> for padding (see the Uniforms struct
// comment below for why that specifically breaks).

struct Uniforms {
    car_x: f32,       // world-space car position, normalized to [-1,1] view space by CPU side
    car_y: f32,
    car_rotation: f32,
    time: f32,
    resolution: vec2<f32>,
    boss_relative_x: f32,
    boss_present: f32,
    boss_telegraphing: f32,
    // car_kind selects body color/silhouette (0=Behemoth, 1=SixWheeler,
    // 2=RedSedan, 3=Pillager). The other two are still plain scalar padding
    // — not vec3<f32>, which has a 16-byte alignment rule in WGSL's uniform
    // address space that silently inflates the struct's required size
    // (confirmed the hard way: wgpu's validator rejected a 48-byte buffer
    // because the shader, with a trailing vec3 pad, actually expected 64).
    car_kind: f32,
    // Surface response from core::material, driven by the car's armour.
    reflectivity: f32,
    specular_power: f32,
};

@group(0) @binding(0)
var<uniform> u: Uniforms;

// The ground profile across the viewport, left edge to right edge, sampled
// CPU-side from `Track::height_at`. Packed 4-per-vec4 because a
// `array<f32>` in uniform space strides at 16 bytes.
//
// Before this the ground was a flat line at a fixed `horizon`. The car
// rotated on slopes and the physics launched it off ramps, but the terrain
// those slopes belong to was never drawn — so the thing the game is about
// was invisible, and a screenshot of a running session is what made that
// obvious.
struct Terrain {
    // Per viewport column: (height, sun visibility, sky openness, unused).
    // The last two are BAKED — traced once when the level loaded, because
    // the camera is fixed and the sun does not move, so they are properties
    // of the terrain and not of the frame.
    cols: array<vec4<f32>, 64>,
};
@group(0) @binding(1)
var<uniform> terrain: Terrain;

/// Ground height in world units at a horizontal screen position.
fn ground_column(x: f32) -> vec4<f32> {
    let t = clamp(x, 0.0, 1.0) * 63.0;
    let i0 = u32(floor(t));
    let i1 = min(i0 + 1u, 63u);
    return mix(terrain.cols[i0], terrain.cols[i1], t - floor(t));
}

fn ground_height(x: f32) -> f32 {
    return ground_column(x).x;
}

/// Terrain surface normal from the height gradient, in screen space.
///
/// Finite differences across one column each way. This is what makes a ramp
/// face read as tilted toward or away from the sun instead of being a flat
/// wash of colour at the same brightness as the ground beside it.
fn ground_normal(x: f32) -> vec2<f32> {
    let d = 1.0 / 63.0;
    let hl = ground_height(x - d);
    let hr = ground_height(x + d);
    // dy/dx in world units per column; the 2*d*span converts to a slope.
    let slope = (hr - hl) * 0.5;
    return normalize(vec2<f32>(-slope, 1.0));
}

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOut {
    // Fullscreen triangle, no vertex buffer needed.
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VertexOut;
    let p = positions[idx];
    out.clip_position = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * 0.5 + vec2<f32>(0.5, 0.5);
    return out;
}

fn hash(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Layered dust streaks scrolling opposite the car's motion direction,
// standing in for kicked-up road dust.
fn dust(uv: vec2<f32>, scroll: f32) -> f32 {
    var v = 0.0;
    var amp = 0.5;
    var p = uv * vec2<f32>(3.0, 8.0) + vec2<f32>(scroll, 0.0);
    for (var i = 0; i < 4; i = i + 1) {
        v = v + noise(p) * amp;
        p = p * 2.0 + vec2<f32>(0.0, 1.7);
        amp = amp * 0.5;
    }
    return v;
}

/// Direction toward the sun in SCREEN space, matching
/// `road_warrior_core::lighting::Sun::default` — ahead and to the right of
/// the driver, so shadows fall back toward the camera where they can be
/// seen. If one of these changes the other has to.
const SUN_DIR = vec2<f32>(0.55, 0.84);
const SUN_COLOR = vec3<f32>(1.05, 0.95, 0.80);
const SKY_COLOR = vec3<f32>(0.55, 0.62, 0.78);

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let aspect = u.resolution.x / max(u.resolution.y, 1.0);

    // --- base scene: ground/sky split ---
    // uv.y is 0 at the bottom of the screen, 1 at the top (wgpu's NDC is
    // Y-up, same as OpenGL) — ground belongs in the low-uv.y branch, sky in
    // the high one. Confirmed by actually running this: the first version
    // had these swapped, putting sky at the bottom of the screen.
    // The car's own resting line, and the vertical scale world units map
    // through. Kept as one expression shared by the terrain and the car so
    // the wheels sit ON the ground they are driving over: the car is placed
    // at `horizon - 0.05 + car_y * 0.02` below, and the ground under it is
    // the same formula with the terrain height in place of car_y.
    let base_line = 0.62 - 0.05;
    let v_scale = 0.02;
    // Surface at this column, and at the car's column for the sky split.
    let col = ground_column(uv.x);
    let raw_h = col.x;
    // Baked, not traced: see the Terrain struct.
    let sun_vis = col.y;
    let sky_open = col.z;
    // A Gap samples as NO_GROUND (-1e4) on the CPU side. Drawing it as a
    // surface at that depth would fill the column with ground; drawing no
    // ground at all is what makes the pit read as a pit, with the walls
    // falling where the gap actually starts and ends.
    let has_ground = raw_h > -100.0;
    let surface = base_line + raw_h * v_scale;
    let horizon = 0.62;
    var color: vec3<f32>;
    if (has_ground && uv.y < surface) {
        let ground_t = uv.y / max(surface, 0.001);
        let dirt_noise = noise(uv * vec2<f32>(40.0 * aspect, 40.0) + vec2<f32>(u.car_x * 2.0, 0.0));
        let albedo = mix(vec3<f32>(0.20, 0.15, 0.11), vec3<f32>(0.36, 0.28, 0.20), ground_t)
            + vec3<f32>(dirt_noise * 0.06);

        // Direct sun, gated by the baked visibility and by how the surface
        // faces. Both matter: a slope can face the sun and still be in the
        // shadow of the ridge above it.
        let n = ground_normal(uv.x);
        let ndotl = max(dot(n, SUN_DIR), 0.0);
        let direct = SUN_COLOR * (ndotl * sun_vis);
        // Sky fill, scaled by how much hemisphere this column can see. This
        // is the term that darkens the floor of a pit at noon.
        let ambient = SKY_COLOR * (0.35 * sky_open);
        color = albedo * (direct + ambient);

        // Depth into the ground darkens, so a tall cut face does not read
        // as uniformly bright rock all the way down.
        color *= mix(0.45, 1.0, clamp(ground_t, 0.0, 1.0));
    } else {
        // Sky gradient still keys off the FIXED horizon, not the surface:
        // gradients that follow the terrain make the sky appear to slide up
        // and down as you drive, which reads as the camera lurching.
        let sky_t = clamp((uv.y - horizon) / (1.0 - horizon), 0.0, 1.0);
        color = mix(vec3<f32>(0.82, 0.80, 0.74), vec3<f32>(0.55, 0.62, 0.68), sky_t);
        // A darker lip right at the surface so a ridge against the sky has
        // an edge instead of fading into it.
        if (has_ground) {
            let edge = smoothstep(0.012, 0.0, uv.y - surface);
            color = mix(color, vec3<f32>(0.16, 0.12, 0.09), edge);
        } else if (uv.y < base_line) {
            // Inside a pit: darken with depth so the hole reads as having a
            // bottom somewhere below, rather than as a window to the sky.
            // Without this the gap looked like the ground simply stopped
            // existing, which is true but does not communicate "fall".
            let depth = clamp((base_line - uv.y) / base_line, 0.0, 1.0);
            color = mix(vec3<f32>(0.10, 0.08, 0.07), vec3<f32>(0.02, 0.02, 0.02), depth);
        }
    }

    // --- car: body + cabin + wheels, shaped/colored per car kind ---
    // The car stays put on screen; the world should scroll underneath it
    // (dirt noise and dust already key off u.car_x for that). A car_screen.x
    // tied directly to car_x was a real bug caught by actually driving
    // across a full-length track: the car ran off the right edge of the
    // screen well before reaching even a third of an 88-unit track, because
    // nothing was translating world position into a bounded screen offset.
    let car_screen = vec2<f32>(0.32, horizon - 0.05 + u.car_y * 0.02);
    let c = cos(-u.car_rotation);
    let s = sin(-u.car_rotation);
    let rel = uv - car_screen;
    // rel.x is pre-multiplied by aspect before rotating, so the rotation
    // itself happens in a uniform (non-stretched) space — this is what
    // makes the wheel circles below render as true circles instead of
    // ellipses, and rotations look like real rotations instead of shears.
    let rel_rot = vec2<f32>(rel.x * aspect * c - rel.y * s, rel.x * aspect * s + rel.y * c);

    // Behemoth is visibly bigger (heavy/armored), RedSedan slightly smaller
    // and sleeker (fast, light) — matches the stat differences in vehicle.rs.
    var size_scale = 1.0;
    if (u.car_kind < 0.5) { size_scale = 1.25; }       // Behemoth
    else if (u.car_kind > 1.5 && u.car_kind < 2.5) { size_scale = 0.85; } // RedSedan

    let body_half = vec2<f32>(0.055, 0.022) * size_scale;
    let cabin_half = vec2<f32>(0.024, 0.016) * size_scale;
    let cabin_offset = vec2<f32>(-0.006, body_half.y + cabin_half.y * 0.7);
    let wheel_radius = 0.013 * size_scale;
    let wheel_y = -body_half.y - wheel_radius * 0.5;
    let wheel_front_x = body_half.x * 0.55;
    let wheel_rear_x = -body_half.x * 0.55;
    let is_six_wheeler = u.car_kind > 0.5 && u.car_kind < 1.5;

    let in_body = abs(rel_rot.x) < body_half.x && abs(rel_rot.y) < body_half.y;
    let cabin_rel = rel_rot - cabin_offset;
    let in_cabin = abs(cabin_rel.x) < cabin_half.x && abs(cabin_rel.y) < cabin_half.y;
    let dist_front_wheel = distance(rel_rot, vec2<f32>(wheel_front_x, wheel_y));
    let dist_rear_wheel = distance(rel_rot, vec2<f32>(wheel_rear_x, wheel_y));
    // A third, centered wheel — the six-wheeler's namesake feature, and the
    // one visual detail that's actually specific to a single car archetype
    // rather than just a size/color tweak.
    let dist_mid_wheel = distance(rel_rot, vec2<f32>(0.0, wheel_y));
    let in_wheel = dist_front_wheel < wheel_radius
        || dist_rear_wheel < wheel_radius
        || (is_six_wheeler && dist_mid_wheel < wheel_radius);

    if (in_body || in_cabin || in_wheel) {
        var albedo: vec3<f32>;
        if (in_wheel) {
            albedo = vec3<f32>(0.05, 0.05, 0.05);
        } else if (u.car_kind < 0.5) {
            albedo = vec3<f32>(0.45, 0.42, 0.40); // Behemoth: gunmetal grey
        } else if (u.car_kind < 1.5) {
            albedo = vec3<f32>(0.35, 0.50, 0.30); // SixWheeler: olive drab
        } else if (u.car_kind < 2.5) {
            albedo = vec3<f32>(0.75, 0.12, 0.10); // RedSedan
        } else {
            albedo = vec3<f32>(0.85, 0.65, 0.15); // Pillager: yellow-gold
        }

        // MATERIAL. Flat fill made the car read as a sticker on the scene:
        // the terrain gained normals and sun/sky terms and the car did not,
        // so it stopped belonging to the same world.
        //
        // The body is a 2D silhouette, so there is no real surface normal.
        // What there IS: the body's own up vector, which rotates with the
        // car, and the position across the body. Treating the body as a
        // cylinder lying along its length gives a normal that sweeps from
        // one edge to the other and rotates with the chassis — enough to
        // read as a rounded panel catching the light, and it means a car
        // pitched nose-up on a ramp visibly changes how it catches the sun.
        // Measured against the region the pixel is actually IN. Using the
        // body's half-height everywhere put the cabin — which sits ABOVE
        // body_half.y — permanently at across = 1, tilting its normal fully
        // away from the sun and rendering it near-black. Caught on the
        // first screenshot: the car had a hole where its cabin should be.
        var across = clamp(rel_rot.y / body_half.y, -1.0, 1.0);
        if (in_cabin) {
            across = clamp(cabin_rel.y / cabin_half.y, -1.0, 1.0);
        }
        let body_up = vec2<f32>(-sin(-u.car_rotation), cos(-u.car_rotation));
        let body_fwd = vec2<f32>(cos(-u.car_rotation), sin(-u.car_rotation));
        let n = normalize(body_up * sqrt(max(1.0 - across * across, 0.0)) + body_fwd * across * 0.35);

        let ndotl = max(dot(n, SUN_DIR), 0.0);
        // The car is above the terrain and sees essentially open sky, so
        // its ambient uses a constant rather than the baked sky term — the
        // bake describes points ON the ground, and sampling it here would
        // dim the car every time it drove past a wall it was not in.
        let ambient = SKY_COLOR * 0.40;
        var lit = albedo * (SUN_COLOR * ndotl + ambient);

        // Specular: a view vector is meaningless in an orthographic 2D
        // scene, so the highlight is placed by the normal alone. Tight for
        // armour, broad for bodywork.
        let spec = pow(ndotl, u.specular_power) * u.reflectivity;
        lit += SUN_COLOR * spec;

        // Sky rim on the up-facing edge, scaled by reflectivity: this is
        // what separates a plated Behemoth from a painted sedan at a glance
        // when both are in shadow.
        let rim = pow(1.0 - abs(across), 3.0) * u.reflectivity * 0.5;
        lit += SKY_COLOR * rim;

        color = lit;
    }

    // --- boss: appears ahead of the car, scaled from world-space distance ---
    // Distance-to-screen scale is tuned so the boss is off-screen until
    // reasonably close, then comes into view as the car approaches it —
    // there's no real depth/perspective here, just a linear proxy for it.
    if (u.boss_present > 0.5) {
        let boss_distance_scale = 0.025;
        let boss_x = car_screen.x + u.boss_relative_x * boss_distance_scale;
        // STANDS ON THE TERRAIN, not at a fixed height. This was
        // `horizon - 0.09`, which was correct while the ground was a flat
        // line and became wrong the moment the ground followed the
        // heightfield — the boss floated over dips and sank into rises.
        // Its feet go on the surface under its own column.
        let boss_half_size = vec2<f32>(0.05, 0.07); // bigger than the car -- reads as a threat
        let boss_ground = base_line + ground_height(boss_x) * v_scale;
        let boss_screen = vec2<f32>(boss_x, boss_ground + boss_half_size.y);
        let boss_rel = uv - boss_screen;
        // Silhouette rather than a bare rectangle: a wide chassis with a
        // raised superstructure, so it reads as a machine at a glance and
        // not as a UI element that wandered into the scene. Still blocky —
        // this matches the car's own deliberately simple shape.
        let b = abs(boss_rel);
        let in_chassis = b.x < boss_half_size.x && boss_rel.y > -boss_half_size.y && boss_rel.y < 0.0;
        let in_tower = b.x < boss_half_size.x * 0.55 && boss_rel.y >= 0.0 && boss_rel.y < boss_half_size.y;
        if (in_chassis || in_tower) {
            // Same cylinder trick as the car, measured against whichever
            // box the pixel is in — the tower sits above the chassis, and
            // sharing one half-height is exactly what rendered the car's
            // cabin black last commit.
            var boss_across: f32;
            if (in_tower) {
                boss_across = clamp(boss_rel.y / boss_half_size.y, -1.0, 1.0);
            } else {
                boss_across = clamp((boss_rel.y + boss_half_size.y * 0.5) / (boss_half_size.y * 0.5), -1.0, 1.0);
            }
            let boss_n = normalize(vec2<f32>(boss_across * 0.5, sqrt(max(1.0 - boss_across * boss_across, 0.0))));
            let boss_ndotl = max(dot(boss_n, SUN_DIR), 0.0);
            // Heavily plated: high reflectivity and a tight highlight,
            // matching what core::material would give an armour stat well
            // past any car's. Hardcoded rather than plumbed through a
            // uniform because the boss has no armour stat to read — adding
            // one to BossData is a data-format change, not a shading one.
            let boss_refl = 0.55;
            let boss_spec = pow(boss_ndotl, 64.0) * boss_refl;

            if (u.boss_telegraphing > 0.5) {
                // Flash between near-white and a saturated amber, not red —
                // confirmed via an actual screenshot that a red/near-white
                // flash was close enough to the car's own red body color to
                // be hard to tell apart at a glance, defeating the point of
                // a telegraph. Amber has no overlap with the car's palette.
                // DELIBERATELY UNLIT. The telegraph is a readability
                // signal, not a surface: an incoming attack has to look the
                // same whether the boss is standing in sun or in the shadow
                // of a ridge. Shading it would make the warning dimmer
                // exactly where the scene is already hard to read.
                let flash = step(0.5, fract(u.time * 8.0));
                color = mix(vec3<f32>(0.95, 0.55, 0.05), vec3<f32>(0.98, 0.92, 0.75), flash);
            } else {
                // Lit, unlike the flash below. Idle purple, shaded the same
                // way as everything else in the scene so it sits in the
                // world rather than on top of it.
                let boss_albedo = vec3<f32>(0.22, 0.14, 0.30);
                var boss_lit = boss_albedo * (SUN_COLOR * boss_ndotl + SKY_COLOR * 0.40);
                boss_lit += SUN_COLOR * boss_spec;
                boss_lit += SKY_COLOR * pow(1.0 - abs(boss_across), 3.0) * boss_refl * 0.5;
                color = boss_lit;
            }
        }
    }

    // --- dust kicked up behind the car ---
    let dust_amt = dust(uv, u.time * 0.6 - u.car_x * 0.3);
    let dust_mask = smoothstep(0.55, 0.0, distance(uv, car_screen) * vec2<f32>(1.0, 2.0).x);
    color = mix(color, vec3<f32>(0.6, 0.55, 0.45), dust_amt * dust_mask * 0.35);

    // --- grain ---
    let grain = hash(uv * u.resolution + vec2<f32>(u.time * 60.0, 0.0));
    color = color + (grain - 0.5) * 0.05;

    // --- vignette ---
    let d = distance(uv, vec2<f32>(0.5, 0.5));
    color = color * (1.0 - smoothstep(0.4, 0.9, d) * 0.5);

    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
