// Sky: fullscreen triangle. Golden-hour jungle atmosphere, weather-driven —
// deep teal zenith, god rays, drifting fbm cloud banks, layered
// mountain silhouettes beyond the map bounds, circling birds, stars.

struct FrameData {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    sun_dir: vec4<f32>,
    cam_pos: vec4<f32>,
    fog_color: vec4<f32>,
    params: vec4<f32>,   // x time, y fog_density, z exposure, w night
    sun_col: vec4<f32>,  // rgb sun colour*intensity, w cloud cover
    weather: vec4<f32>,  // x rain, y snow, z wetness, w lightning
    wind: vec4<f32>,
    event: vec4<f32>,    // x flood rise, y quake shake, z fire glow, w unused
};

@group(0) @binding(0) var<uniform> frame: FrameData;

struct VSOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    var p = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0)
    );
    var out: VSOut;
    out.clip = vec4(p[vi], 0.99999, 1.0);
    out.ndc = p[vi];
    return out;
}

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2(123.34, 456.21));
    q = q + dot(q, q + 45.32);
    return fract(q.x * q.y);
}

fn noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2(1.0, 0.0));
    let c = hash21(i + vec2(0.0, 1.0));
    let d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i: i32 = 0; i < 5; i = i + 1) {
        v = v + a * noise2(q);
        q = q * 2.03 + vec2(11.7, 5.3);
        a = a * 0.5;
    }
    return v;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    // reconstruct view ray
    let ndc = vec4<f32>(in.ndc, 0.5, 1.0);
    let world_p = frame.inv_view_proj * ndc;
    let ray = normalize(world_p.xyz / world_p.w - frame.cam_pos.xyz);

    let sun = normalize(frame.sun_dir.xyz);
    let h = clamp(ray.y, -1.0, 1.0);
    let t = frame.params.x;
    let night = frame.params.w;
    let cloud = frame.sun_col.w;
    let flash = frame.weather.w;
    let rain = frame.weather.x;
    // the spirit sky of Starwell Glacier: above the clouds there are no
    // mountains, the stars come down to the horizon, and the aurora walks
    let spirit = frame.sun_dir.w;

    // --- sky gradient: deep jungle-teal zenith → hot golden horizon,
    // greying under cloud, sinking to indigo for the Long Night
    let grey_z = vec3(0.16, 0.18, 0.22);
    let grey_m = vec3(0.34, 0.36, 0.40);
    let grey_h = vec3(0.50, 0.50, 0.50);
    let zenith = mix(mix(vec3(0.045, 0.10, 0.20), grey_z, cloud * 0.8), vec3(0.01, 0.012, 0.035), night);
    let mid = mix(mix(vec3(0.30, 0.33, 0.45), grey_m, cloud * 0.8), vec3(0.03, 0.035, 0.08), night);
    let horizon = mix(mix(vec3(0.96, 0.60, 0.32), grey_h, cloud * 0.85), vec3(0.08, 0.06, 0.14), night);
    var col: vec3<f32>;
    if (h < 0.16) {
        col = mix(horizon, mid, smoothstep(-0.06, 0.16, h));
    } else {
        col = mix(mid, zenith, smoothstep(0.16, 0.85, h));
    }

    // --- sun disc + layered bloom (a cold moon on the Long Night)
    let d = max(dot(ray, sun), 0.0);
    let sun_vis = (1.0 - cloud * 0.9);
    let disc = mix(vec3(1.0, 0.74, 0.42), vec3(0.75, 0.8, 1.0), night);
    col = col + disc * pow(d, 240.0) * mix(3.2, 1.6, night) * sun_vis;
    col = col + mix(vec3(1.0, 0.58, 0.30), vec3(0.3, 0.35, 0.6), night) * pow(d, 10.0) * 0.42 * sun_vis;
    col = col + mix(vec3(1.0, 0.50, 0.26), vec3(0.15, 0.17, 0.3), night) * pow(d, 2.4) * 0.15 * sun_vis;

    // --- god rays: slowly breathing angular shafts around the sun
    let cosang = d;
    if (cosang > 0.05) {
        let ra = atan2(ray.z, ray.x);
        let sa = atan2(sun.z, sun.x);
        let da = ra - sa;
        let streak = pow(0.5 + 0.5 * sin(da * 21.0 + t * 0.05), 3.0)
            * (0.6 + 0.4 * sin(da * 8.0 - t * 0.033));
        col = col + vec3(1.0, 0.62, 0.32) * pow(cosang, 2.6) * streak * 0.34 * smoothstep(0.55, 0.0, h) * (1.0 - cloud) * (1.0 - night);
    }

    // --- drifting cloud banks (two parallax layers, sun-lit edges).
    // Coverage threshold slides with the weather: clear skies keep a few
    // banks, overcast closes to a lid; wind speed drives the drift.
    if (h > 0.02) {
        let lit = smoothstep(0.25, 0.9, cosang) * (1.0 - night * 0.9);
        let wv = frame.wind.xz * 0.0025;
        let lo1 = mix(0.46, 0.12, cloud);
        let hi1 = mix(0.74, 0.40, cloud);
        let p1 = ray.xz / (ray.y + 0.18) * 1.35 + vec2<f32>(t * 0.009, t * 0.004) + wv * t;
        let c1 = fbm(p1);
        let cov1 = smoothstep(lo1, hi1, c1);
        let dark1 = mix(vec3(0.30, 0.27, 0.37), vec3(0.20, 0.20, 0.24), cloud);
        let shade1 = mix(mix(dark1, vec3(1.0, 0.72, 0.48), lit), vec3(0.04, 0.045, 0.08), night);
        col = mix(col, shade1 + vec3(0.8) * flash, cov1 * mix(0.72, 0.95, cloud) * smoothstep(0.02, 0.16, h));
        let lo2 = mix(0.58, 0.25, cloud);
        let hi2 = mix(0.9, 0.55, cloud);
        let p2 = ray.xz / (ray.y + 0.34) * 0.62 + vec2<f32>(t * 0.004 + 7.0, -t * 0.003) + wv * t * 0.6;
        let c2 = fbm(p2);
        let cov2 = smoothstep(lo2, hi2, c2);
        let dark2 = mix(vec3(0.36, 0.31, 0.41), vec3(0.16, 0.16, 0.20), cloud);
        let shade2 = mix(mix(dark2, vec3(0.95, 0.66, 0.42), lit * 0.8), vec3(0.03, 0.035, 0.07), night);
        col = mix(col, shade2 + vec3(0.6) * flash, cov2 * mix(0.45, 0.8, cloud) * smoothstep(0.06, 0.3, h));
    }
    // rain veil: a grey sheet low on the horizon
    col = mix(col, vec3(0.42, 0.44, 0.48) * (1.0 - night * 0.85), rain * 0.35 * smoothstep(0.35, 0.0, h));

    // --- aurora: tall curtains along the sky, teal to violet, slowly
    // walking and rippling (spirit sky only)
    let ra = atan2(ray.z, ray.x);
    if (spirit > 0.5 && h > 0.0) {
        let wave = fbm(vec2(ra * 2.3 + t * 0.012, t * 0.02)) * 1.8;
        let fold = abs(sin(ra * 5.0 + wave * 3.0 + t * 0.03));
        let curtain = pow(1.0 - fold, 6.0) + 0.35 * pow(1.0 - abs(sin(ra * 11.0 + wave * 5.0)), 10.0);
        let band = smoothstep(0.06, 0.26, h) * (1.0 - smoothstep(0.34 + wave * 0.12, 0.75, h));
        let flick = 0.75 + 0.25 * fbm(vec2(ra * 30.0, h * 40.0 - t * 0.6));
        let aur = mix(vec3(0.15, 0.95, 0.70), vec3(0.62, 0.35, 1.0), smoothstep(0.22, 0.6, h));
        col = col + aur * curtain * band * flick * 0.55 * (1.0 - cloud * 0.8);
    }

    // --- distant mountain silhouettes beyond the physical terrain ring
    // (not in the spirit sky: there, the far horizon is the cloud sea's
    // glow)
    let r1 = fbm(vec2(ra * 3.1, 4.2)) * 0.10 + 0.050;
    let r2 = fbm(vec2(ra * 4.7, 9.1)) * 0.085 + 0.030;
    let r3 = fbm(vec2(ra * 7.3, 17.0)) * 0.05 + 0.015;
    let nd = 1.0 - night * 0.85;
    if (spirit > 0.5) {
        // below the horizon there is no ground: the cloud deck lies far
        // beneath the islands (y -260), billows lit pearl by the moon over a
        // deep violet void, with a thinner layer of wisps drifting between
        // (y -110) the other way; both melt into haze toward the horizon.
        // Ray-plane intersections from the real camera, so the deck sits
        // where it should at any camera height.
        if (h < 0.02) {
            let cam = frame.cam_pos.xyz;
            let dy = min(ray.y, -0.004);
            let t1 = (-260.0 - cam.y) / dy;
            let p1 = (cam.xz + ray.xz * t1) * 0.0045 + vec2(t * 0.0040, t * 0.0015);
            let c1 = fbm(p1);
            let dens1 = smoothstep(0.34, 0.72, c1);
            let billow = mix(vec3(0.40, 0.42, 0.66), vec3(0.94, 0.93, 1.0), smoothstep(0.42, 0.86, c1));
            var dc = mix(vec3(0.13, 0.13, 0.30), billow, dens1);
            let t2 = (-110.0 - cam.y) / dy;
            let p2 = (cam.xz + ray.xz * t2) * 0.011 + vec2(-t * 0.010, t * 0.006);
            let c2 = fbm(p2);
            dc = mix(dc, vec3(0.86, 0.87, 0.98), smoothstep(0.56, 0.86, c2) * 0.65);
            let far = 1.0 - exp(-t1 * 0.0011);
            dc = mix(dc, vec3(0.72, 0.72, 0.92), far);
            col = mix(col, dc * (1.0 - night * 0.4), smoothstep(0.02, -0.03, h));
        }
    } else {
        if (h < r1) {
            col = mix(col, mix(vec3(0.36, 0.37, 0.44), frame.fog_color.xyz, 0.35) * nd, 0.82); // far — sun haze
        }
        if (h < r2) {
            col = mix(col, vec3(0.23, 0.27, 0.31) * nd, 0.86);
        }
        if (h < r3) {
            col = mix(col, vec3(0.11, 0.155, 0.145) * nd, 0.92); // near — jungle dark
        }
    }

    // --- circling birds (tiny flapping silhouettes)
    for (var bi: i32 = 0; bi < 3 - 3 * i32(spirit); bi = bi + 1) {
        let fb = f32(bi);
        let ba = t * (0.05 + fb * 0.013) + fb * 2.1;
        // lightning scatters the flock upward; night grounds it
        let be = 0.34 + 0.06 * sin(t * 0.21 + fb) + flash * 0.12 - night * 0.6;
        let bdir = vec3(cos(ba) * cos(be), sin(be), sin(ba) * cos(be));
        let dd = distance(ray, normalize(bdir));
        let flap = 0.003 + 0.0035 * abs(sin(t * 10.0 + fb * 2.0));
        if (dd < flap) {
            col = mix(col, vec3(0.05, 0.05, 0.06), 0.85);
        }
    }

    // --- wildfire: smoke stains the low sky orange-brown
    col = mix(col, vec3(0.45, 0.25, 0.12), frame.event.z * 0.5 * smoothstep(0.4, -0.05, h));

    // --- lightning: the whole sky flashes, brightest toward the horizon
    col = col + vec3(0.7, 0.72, 0.95) * flash * (0.35 + 0.65 * smoothstep(0.6, 0.0, h));

    // --- stars high up (the Long Night brings them down to the horizon)
    // (the spirit sky: stars all the way down, more of them, and some warm)
    let floor_h = mix(0.28 - night * 0.22, 0.0, spirit);
    if (h > floor_h) {
        let sp = ray.xz / (ray.y + 0.4) * 24.0;
        let cell = floor(sp);
        let f = hash21(cell);
        if (f > mix(0.975, 0.955, spirit)) {
            let tw = 0.5 + 0.5 * sin(t * 2.2 + f * 90.0);
            let dist2 = length(fract(sp) - vec2(0.5));
            let lift = max(h - floor_h, 0.0) + spirit * 0.25;
            let star = smoothstep(0.28, 0.0, dist2) * lift * mix(2.2, 5.5, night) * tw * (1.0 - cloud);
            let warm = step(0.992, f) * spirit;
            col = col + mix(vec3(0.9, 0.95, 1.0), vec3(1.0, 0.85, 0.55), warm) * star;
        }
    }

    // --- horizon haze band melts into the world fog
    // (the spirit sky's deck carries its own haze: fogging it again washed
    // the clouds out)
    col = mix(col, frame.fog_color.xyz, smoothstep(0.20, -0.05, h) * 0.6 * (1.0 - spirit * 0.75));
    return vec4(col, 1.0);
}
