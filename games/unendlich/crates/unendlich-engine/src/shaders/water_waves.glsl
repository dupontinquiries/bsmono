// water_waves.glsl — physics-driven analytic water surface waves.
//
// SINGLE SOURCE OF TRUTH. The wave field is a PURE function of
//   (world XZ, time, wind vector + magnitude, temperature)
// with no texture reads and no GPU-only state. That is deliberate:
//   * cheap on a tile GPU — no dependent texture fetches, all ALU, and the
//     per-octave wavenumber/speed fold to constants so only the phase varies;
//   * the analytic gradient gives the surface normal in ONE evaluation (vs a
//     3-tap finite difference), so it is ~3x cheaper than the usual approach;
//   * a CPU port is a direct transcription — see RULESET §water. That port is
//     what entity physics (buoyancy, splashes, wakes) will call so floating
//     objects agree with the rendered surface. KEEP THE TWO IN LOCKSTEP.
//
// Wired to the engine's live fields (this is the "physics drives the waves"
// part, not a generic FBM):
//   wind direction   -> dominant wave propagation direction
//   wind magnitude   -> amplitude + speed (calm glassy <-> windy chop)
//   temperature      -> damping toward glassy as water nears freezing, so the
//                       surface calms just before the freeze system turns it to
//                       ice (ties into chunk_generation's freeze band).
// Future inputs to the same WaveParams (documented hooks, not yet wired):
//   humidity / weather (rain choppiness + ripple rings), season, and an
//   entity-ripple displacement term for objects pushing the surface around.

#ifndef WATER_WAVES_GLSL
#define WATER_WAVES_GLSL

// A/B toggle (flip + hot-reload), matching the SHOW_UV_DEBUG const convention.
const bool  WATER_WAVE_NORMALS = true;

const int   WW_OCTAVES     = 3;      // r428: was 4; dropped the ~1.1Hz octave that read as buzz
const float WW_TAU         = 6.28318530718;
const float WW_GRAVITY     = 9.8;
const float WW_BASE_LAMBDA = 5.5;    // metres, longest wave
const float WW_LACUNARITY  = 1.62;   // wavelength shrinks per octave
const float WW_PERSISTENCE = 0.55;   // amplitude decays per octave
const float WW_BASE_AMP    = 0.09;   // metres, crest of the longest wave at full wind
const float WW_FAN         = 0.5;    // radians, octave direction spread around wind

// Large-scale "swell": a slow, low-frequency amplitude modulation that drifts
// downwind, so the chop isn't spatially uniform — calm patches and choppy
// patches. Folded into the per-octave amplitude (it scales how rippled each
// region is); its own gentle slope is intentionally NOT added to the normal,
// which should track the local ripples, not the ~200 m swell.
const float WW_SWELL_FREQ  = 0.03;   // ~210 m swell wavelength
const float WW_SWELL_SPEED = 0.25;
const float WW_SWELL_MIN   = 0.45;   // calm patches keep 45% amplitude

// Per-octave constant phase offset so the octaves don't all align at t=0.
// Pure arithmetic (no array, no hash) so the CPU port is unambiguous.
float ww_phase_offset(int i) { return float(i) * 2.3994; }

// Resolved wave inputs. Kept tiny so the CPU mirror is a 1:1 struct.
struct WaveParams {
    vec2  dir;          // normalized dominant direction (from wind)
    float amp_scale;    // 0..1 overall amplitude (wind up, freezing down)
    float speed_scale;  // dispersion-speed multiplier (wind up)
    float t;            // seconds
};

WaveParams water_wave_params(vec2 wind_xy, float wind_mag, float temp, float t) {
    WaveParams p;
    float wm = length(wind_xy);
    p.dir = wm > 1e-4 ? wind_xy / wm : vec2(0.70710678, 0.70710678);
    float wind = clamp(wind_mag, 0.0, 2.0);
    float thaw = smoothstep(0.05, 0.22, clamp(temp, 0.0, 1.0)); // calm near freeze
    p.amp_scale   = (0.30 + 0.70 * smoothstep(0.0, 1.0, wind)) * mix(0.12, 1.0, thaw);
    p.speed_scale = 0.60 + 0.50 * wind;
    p.t = t;
    return p;
}

mat2 ww_rot(float a) { float c = cos(a), s = sin(a); return mat2(c, s, -s, c); }

float ww_swell(vec2 pos, WaveParams p) {
    float phase = dot(p.dir, pos) * WW_SWELL_FREQ - p.t * WW_SWELL_SPEED;
    return mix(WW_SWELL_MIN, 1.0, 0.5 + 0.5 * sin(phase));
}

// Wave height AND its analytic XZ gradient in a single pass.
// Returns vec3(height, dHeight/dx, dHeight/dz). Height is in metres.
vec3 water_wave_height_grad(vec2 pos, WaveParams p) {
    float height = 0.0, dHx = 0.0, dHz = 0.0;
    float lambda = WW_BASE_LAMBDA;
    float amp    = WW_BASE_AMP * p.amp_scale * ww_swell(pos, p);

    for (int i = 0; i < WW_OCTAVES; ++i) {
        float k = WW_TAU / lambda;                 // wavenumber (constant-folds)
        float w = sqrt(WW_GRAVITY * k) * p.speed_scale; // deep-water dispersion
        float ang = WW_FAN * (float(i) - float(WW_OCTAVES - 1) * 0.5); // symmetric fan, any count
        vec2  d = ww_rot(ang) * p.dir;

        float phase = k * dot(d, pos) - w * p.t + ww_phase_offset(i);
        float s = sin(phase);
        float g = 0.5 + 0.5 * s;                   // [0,1]
        height += amp * g * g;                     // squared sine -> sharper crests
        // d(amp * g^2)/dphase = amp * 2g * (0.5*cos) = amp * g * cos
        float dh = amp * g * cos(phase);
        dHx += dh * k * d.x;
        dHz += dh * k * d.y;

        lambda /= WW_LACUNARITY;
        amp    *= WW_PERSISTENCE;
    }
    return vec3(height, dHx, dHz);
}

float water_wave_height(vec2 pos, WaveParams p) {
    return water_wave_height_grad(pos, p).x;
}

// World-space normal for an up-facing water surface. `grazing` in [0,1] is
// |dot(flat_up_normal, view_dir)|: slope is damped at grazing angles to kill
// shimmer (and reduce specular aliasing) — sharp ripples only where you look
// down into the water.
vec3 water_wave_normal(vec2 pos, WaveParams p, float grazing) {
    vec3 hg = water_wave_height_grad(pos, p);
    float infl = mix(0.15, 1.0, grazing);
    return normalize(vec3(-hg.y * infl, 1.0, -hg.z * infl));
}

#endif // WATER_WAVES_GLSL
