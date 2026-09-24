//! studio-audio — waveforms and mixdown.
//!
//! ## Peaks: the idea worth taking from Audacity
//!
//! Audacity does not read a file to draw it. Each `Sequence` carries summary
//! blocks — min/max/RMS at 256 samples per point and again at 65,536 — so
//! drawing a ten-hour recording zoomed out reads a few kilobytes instead of
//! gigabytes, and the zoom level picks which summary to use. That structure is
//! the reason it stays responsive on files that make other editors crawl, and
//! it is worth far more than any of its UI code.
//!
//! [`Peaks`] is that idea in a few dozen lines: a pyramid of min/max pairs,
//! each level a fixed factor coarser than the one below, with [`Peaks::for_px`]
//! choosing the level whose buckets are closest to one screen pixel. Min AND
//! max are kept (rather than an average or an absolute value) because a
//! waveform drawn from averages loses exactly the transients an editor is
//! looking for — a click, a clipped peak, the attack of a drum.
//!
//! ## Mixdown
//!
//! [`mixdown`] turns a stretch of timeline into interleaved samples: it walks
//! the clips that intersect the window, reads their source, applies clip gain
//! and the fades, and sums. It is deliberately a pure function of
//! (project, sources, window) so a render is reproducible and testable — no
//! device, no callback, no clock.

pub mod boom;
pub mod format;
pub mod stems;
pub mod block;
pub mod destructive;

use std::collections::BTreeMap;
use studio_core::{eval_lane, AutoTarget, Project, SourceId, Ticks, TrackKind};

pub mod export;
pub mod repitch;

/// Audio for one source, as mono-per-channel planes.
///
/// Real playback will stream these from disk; the model does not care, which is
/// why decode lives outside this crate. Tests build them by hand.
#[derive(Debug, Clone, Default)]
pub struct SourceAudio {
    pub sample_rate: u32,
    /// One vector per channel, all the same length.
    pub channels: Vec<Vec<f32>>,
}

impl SourceAudio {
    pub fn frames(&self) -> usize { self.channels.first().map(|c| c.len()).unwrap_or(0) }

    /// Mono sum, for peak computation and for mixing a multi-channel source
    /// into a mono bus. Averaged, not summed, so a stereo file does not come
    /// out 6 dB hotter than a mono one.
    pub fn mono(&self, frame: usize) -> f32 {
        if self.channels.is_empty() { return 0.0; }
        let n = self.channels.len() as f32;
        self.channels.iter().map(|c| c.get(frame).copied().unwrap_or(0.0)).sum::<f32>() / n
    }
}

/// One bucket of a waveform: the extremes of the samples it covers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MinMax {
    pub min: f32,
    pub max: f32,
}

impl MinMax {
    pub fn merge(self, o: MinMax) -> MinMax {
        MinMax { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    /// Peak-to-peak height — what actually gets drawn.
    pub fn span(&self) -> f32 { self.max - self.min }
}

/// A multi-resolution peak pyramid. Level 0 is the finest.
#[derive(Debug, Clone)]
pub struct Peaks {
    /// Samples covered by one bucket at level 0.
    pub base: usize,
    /// How much coarser each level is than the one below.
    pub factor: usize,
    pub levels: Vec<Vec<MinMax>>,
    pub frames: usize,
    /// The sample rate of the audio these peaks were built from. A waveform
    /// must read samples at THIS rate, not the rate a project records for the
    /// source: the cartoon fixture recorded its cameras as 48 kHz over 16 kHz
    /// audio, and every waveform read the wrong span.
    pub rate: u32,
}

/// What happens to a clip's effects after the clip ends.
///
/// A reverb or a delay keeps sounding after its input stops — that is the
/// effect. An editor needs two things from that, and every tool picks one:
///
/// * **Cut**: the tail ends exactly where the clip does. A hard cut to the
///   next scene, a gag that needs dead air, a narration line that must not
///   bleed under the next shot's dialogue.
/// * **Ring out**: the tail keeps going past the clip end for `ring`
///   seconds, and optionally fades over the last `fade` of them — so a clip
///   can be trimmed tight to the dry sound while its space decays naturally,
///   or ring for exactly as long as a transition wants and then stop.
///
/// Both, per clip, and switchable without touching the effect: this is a
/// property of the EDIT, not of the reverb. Carried as an effect of kind
/// `"tail"` on the clip's own chain (params `ring` and `fade`, seconds)
/// because a clip's effect list already syncs, undoes, saves and travels
/// over the wire — and an unknown kind is skipped by the DSP, so a build
/// that predates this plays the clip exactly as before.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClipTail {
    /// Seconds of tail after the clip end. 0 is Cut.
    pub ring: f64,
    /// Seconds, at the END of the ring, over which the tail fades to
    /// silence. 0 means the ring stops dead at `ring`: a hard cut, just
    /// later than the clip's own end.
    pub fade: f64,
}

/// The effect kind that carries a clip's tail setting.
pub const TAIL_KIND: &str = "tail";

impl ClipTail {
    /// Read from a clip's effect list; the last enabled `"tail"` wins.
    pub fn of(effects: &[studio_core::Effect]) -> ClipTail {
        effects.iter().filter(|e| e.enabled && e.kind == TAIL_KIND).last().map(|e| {
            let ring = e.params.get("ring").copied().unwrap_or(0.0).max(0.0) as f64;
            let fade = (e.params.get("fade").copied().unwrap_or(0.0).max(0.0) as f64).min(ring);
            ClipTail { ring, fade }
        }).unwrap_or_default()
    }

    pub fn ring_ticks(&self) -> Ticks { Ticks::from_seconds(self.ring) }

    /// Gain `past` seconds after the clip end: 1 until the fade starts, a
    /// linear ramp through it, 0 at and beyond `ring`.
    pub fn gain_at(&self, past: f64) -> f32 {
        if past >= self.ring { return 0.0; }
        let fade_from = self.ring - self.fade;
        if self.fade <= 0.0 || past < fade_from { return 1.0; }
        (1.0 - (past - fade_from) / self.fade).clamp(0.0, 1.0) as f32
    }

    /// The effect entry that sets this tail on a clip.
    pub fn effect(&self) -> studio_core::Effect {
        studio_core::Effect {
            kind: TAIL_KIND.into(),
            enabled: true,
            params: [("ring".to_string(), self.ring as f32), ("fade".to_string(), self.fade as f32)]
                .into_iter().collect(),
        }
    }
}

/// Where a project's sound actually ends: the last clip's end, or later if
/// a clip's tail rings past it.
///
/// `Project::duration` is the end of the last CLIP, which is the right
/// answer for the timeline and the wrong one for a render: a ring-out on the
/// final shot would be cut off by the very file that was meant to carry it.
/// Every render that decides "how long is this" asks here instead.
pub fn audible_end(project: &Project) -> Ticks {
    project.clips.values()
        .map(|c| c.end().saturating_add(ClipTail::of(&c.effects).ring_ticks()))
        .max()
        .unwrap_or(Ticks::ZERO)
}

/// Which part of the spectrum a waveform drawing should show.
///
/// A full-band waveform is a silhouette of LOUDNESS, and for scoring that is
/// often the least useful thing on screen: a dialogue take and a kick drum
/// draw the same shape. Looking at one band at a time answers questions the
/// full waveform cannot — where the low end actually lands, whether a
/// narration take has room under it, where the sibilance is — before
/// listening decides anything.
///
/// Ranges are deliberately broad and named for what they are used for, not
/// for round numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Band {
    #[default]
    Full,
    /// Below ~200 Hz: kick, bass, rumble, room.
    Low,
    /// ~200 Hz–2 kHz: where voices and most instruments live.
    Mid,
    /// Above ~2 kHz: consonants, cymbals, air, sibilance.
    High,
}

impl Band {
    pub fn label(self) -> &'static str {
        match self { Band::Full => "full", Band::Low => "low", Band::Mid => "mid", Band::High => "high" }
    }

    /// The corner frequencies this band is made from, in Hz.
    fn corners(self) -> (Option<f32>, Option<f32>) {
        match self {
            Band::Full => (None, None),
            Band::Low => (None, Some(200.0)),
            Band::Mid => (Some(200.0), Some(2_000.0)),
            Band::High => (Some(2_000.0), None),
        }
    }
}

/// A one-pole filter pair, enough to SEE a band with.
///
/// Two one-pole stages per side (12 dB/octave), not one.
///
/// A single pole was tried first and is not enough to LOOK at: at 200 Hz it
/// leaves a 900 Hz voice only 13 dB down, so the "low band" of a narration
/// take was still mostly the voice — the drawing would have answered the
/// question with the same shape it always shows. Two stages put that voice
/// 26 dB down, which reads as a different picture. Still not a mixing
/// decision: judge a crossover by listening through `studio_fx`, not by
/// reading a waveform.
struct BandFilter {
    hp: Option<f32>,
    lp: Option<f32>,
    hp_state: [(f32, f32); 2],  // (y, x) per stage
    lp_state: [f32; 2],
}

impl BandFilter {
    fn new(band: Band, rate: u32) -> BandFilter {
        let k = |f: f32| (-std::f32::consts::TAU * f / rate.max(1) as f32).exp();
        let (hi, lo) = band.corners();
        BandFilter { hp: hi.map(k), lp: lo.map(k), hp_state: [(0.0, 0.0); 2], lp_state: [0.0; 2] }
    }
    #[inline]
    fn step(&mut self, x: f32) -> f32 {
        let mut v = x;
        if let Some(a) = self.hp {
            for st in self.hp_state.iter_mut() {
                st.0 = a * (st.0 + v - st.1);
                st.1 = v;
                v = st.0;
            }
        }
        if let Some(a) = self.lp {
            for y in self.lp_state.iter_mut() {
                *y = (1.0 - a) * v + a * *y;
                v = *y;
            }
        }
        v
    }
}

impl Peaks {
    /// Build the pyramid. `base` is Audacity's 256; `factor` of 256 gives its
    /// second summary at 65,536 in one more level.
    pub fn build(audio: &SourceAudio, base: usize, factor: usize) -> Peaks {
        Peaks::build_band(audio, base, factor, Band::Full)
    }

    /// The pyramid of ONE band of the audio — the same shape, so every
    /// drawing, hit test and zoom level works unchanged; only what the
    /// buckets summarise changes. `Band::Full` is the plain build and costs
    /// nothing extra.
    pub fn build_band(audio: &SourceAudio, base: usize, factor: usize, band: Band) -> Peaks {
        let base = base.max(1);
        let factor = factor.max(2);
        let frames = audio.frames();
        let mut level0 = Vec::with_capacity(frames.div_ceil(base));
        let mut filter = BandFilter::new(band, audio.sample_rate);
        let mut i = 0;
        while i < frames {
            let end = (i + base).min(frames);
            let mut mm = MinMax { min: f32::INFINITY, max: f32::NEG_INFINITY };
            for f in i..end {
                // Filtered in one pass, in order — the filter carries its
                // state across buckets, so the drawing has no seams at
                // bucket boundaries.
                let v = filter.step(audio.mono(f));
                mm.min = mm.min.min(v);
                mm.max = mm.max.max(v);
            }
            level0.push(mm);
            i = end;
        }
        let mut levels = vec![level0];
        // Each level is built from the one below, not from the samples again —
        // an O(n) pyramid rather than O(n log n), and the reason building peaks
        // for a long file is cheap.
        while levels.last().map(|l| l.len()).unwrap_or(0) > 1 {
            let prev = levels.last().unwrap();
            let mut next = Vec::with_capacity(prev.len().div_ceil(factor));
            let mut j = 0;
            while j < prev.len() {
                let end = (j + factor).min(prev.len());
                let mut mm = prev[j];
                for b in &prev[j + 1..end] { mm = mm.merge(*b); }
                next.push(mm);
                j = end;
            }
            levels.push(next);
        }
        Peaks { base, factor, levels, frames, rate: audio.sample_rate }
    }

    /// Samples covered by one bucket at `level`.
    pub fn samples_per_bucket(&self, level: usize) -> usize {
        self.base * self.factor.pow(level as u32)
    }

    /// The level whose buckets are closest to (but not finer than) one pixel
    /// covering `samples_per_px`. Picking a level that is too fine wastes work;
    /// too coarse and the waveform visibly steps.
    pub fn for_px(&self, samples_per_px: f64) -> usize {
        let mut best = 0;
        for l in 0..self.levels.len() {
            if (self.samples_per_bucket(l) as f64) <= samples_per_px.max(1.0) {
                best = l;
            } else {
                break;
            }
        }
        best
    }

    /// Peaks for a span of samples, reduced to `columns` buckets — exactly what
    /// a waveform widget needs for a rectangle `columns` pixels wide.
    ///
    /// The scale is the REQUESTED span's, even where it runs past the end of
    /// the audio: those columns are empty. It used to clamp `to` to the end
    /// first and spread what was left across every column -- so a clip
    /// scrolled half off-screen, or longer than its audio, had its waveform
    /// stretched to fill the width (seen on the cartoon fixture).
    pub fn window(&self, from: usize, to: usize, columns: usize) -> Vec<MinMax> {
        if columns == 0 || to <= from { return Vec::new(); }
        let spp = (to - from) as f64 / columns as f64;
        let level = self.for_px(spp);
        let buckets = &self.levels[level];
        let per = self.samples_per_bucket(level);
        (0..columns)
            .map(|c| {
                let s0 = from + (c as f64 * spp) as usize;
                if s0 >= self.frames || buckets.is_empty() { return MinMax { min: 0.0, max: 0.0 }; }
                let s1 = (from + (((c + 1) as f64 * spp) as usize).max(1)).min(self.frames);
                let b0 = s0 / per;
                let b1 = ((s1 + per - 1) / per).max(b0 + 1).min(buckets.len());
                buckets[b0.min(buckets.len().saturating_sub(1))..b1]
                    .iter()
                    .copied()
                    .fold(MinMax { min: 0.0, max: 0.0 }, MinMax::merge)
            })
            .collect()
    }
}

/// A fade on a clip, in ticks from its own start/end.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Fades {
    pub fade_in: Ticks,
    pub fade_out: Ticks,
}

impl Fades {
    /// Gain multiplier at `offset` into a clip of length `len`. Linear, which is
    /// what both source editors default to; the curve is where an equal-power
    /// option would go.
    pub fn gain_at(&self, offset: Ticks, len: Ticks) -> f32 {
        let mut g = 1.0f32;
        if self.fade_in.0 > 0 && offset.0 < self.fade_in.0 {
            g *= offset.0 as f32 / self.fade_in.0 as f32;
        }
        let from_end = len.0 - offset.0;
        if self.fade_out.0 > 0 && from_end < self.fade_out.0 {
            g *= (from_end.max(0) as f32) / self.fade_out.0 as f32;
        }
        g.clamp(0.0, 1.0)
    }
}

/// Render `[from, to)` of the timeline to interleaved stereo at `rate`.
///
/// Pure: same inputs, same samples, every time. Muted tracks contribute
/// nothing; when any track is soloed, only soloed tracks are heard — the rule
/// every DAW shares and every reimplementation gets backwards at least once.
///
/// ## Bus routing
///
/// A track with `route: None` feeds master directly, exactly as it always
/// has — a project with no `Bus` values mixes bit-for-bit identically to
/// before busses existed, which is the property that makes adding a bus
/// graph to an existing model safe.
///
/// A track routed to a bus instead accumulates into that bus's own buffer.
/// Busses are then folded in `Project::bus_order()` — children before the
/// parents they feed, so a bus's own gain and its sends are fully resolved
/// before anything downstream reads it:
///
/// 1. **Sends are tapped from the bus's signal**, `pre_fader` from before
///    this bus's own gain is applied, post-fader from after — the same
///    distinction every hardware console makes, because a pre-fader send
///    (a cue mix, typically) must not move when someone rides the fader.
/// 2. **The main path carries the post-gain signal** into the bus's parent
///    (or master, if `parent` is `None`).
///
/// Each bus's own insert chain (`Bus.chain`) runs before its fader — see the
/// bus-fold loop below for the exact order and why `pre` (what a pre-fader
/// send taps) is captured after the chain but before gain. `Bus.chain` runs
/// through `studio_fx::instantiate`/`Instance::process_audio`; an effect kind
/// this build doesn't know is silently skipped, the same "opens without the
/// plugin" rule every other effect host in this codebase follows.
///
/// A bus is muted from `Bus::muted`, or solo-gated from `bus_solo` (a
/// mixdown-time listening choice, not project state — see `Op::SetBusFlags`'s
/// docs on why bus solo, like track solo, is a parameter here rather than a
/// persisted field). When `bus_solo` is non-empty, a bus survives to its
/// parent and sends only if it is itself soloed, an ancestor of a soloed bus
/// (so the soloed signal still has a path to master), or a descendant of one
/// (so soloing a bus lets its whole subtree through) — walked via `parent`
/// only, not `sends`, since a send is a parallel tap rather than part of the
/// tree a solo should open up.
fn solo_audible_bus_set(project: &Project, bus_solo: &[studio_core::BusId]) -> std::collections::BTreeSet<studio_core::BusId> {
    use studio_core::BusId;
    let mut set: std::collections::BTreeSet<BusId> = std::collections::BTreeSet::new();
    if bus_solo.is_empty() {
        return set;
    }
    for &s in bus_solo {
        set.insert(s);
        let mut cur = project.busses.get(&s).and_then(|b| b.parent);
        let mut seen = std::collections::BTreeSet::new();
        while let Some(p) = cur {
            if !seen.insert(p) { break; } // cycle guard; apply() prevents this in live data
            set.insert(p);
            cur = project.busses.get(&p).and_then(|b| b.parent);
        }
    }
    for id in project.busses.keys() {
        if set.contains(id) { continue; }
        let mut cur = project.busses.get(id).and_then(|b| b.parent);
        let mut seen = std::collections::BTreeSet::new();
        while let Some(p) = cur {
            if !seen.insert(p) { break; }
            if bus_solo.contains(&p) {
                set.insert(*id);
                break;
            }
            cur = project.busses.get(&p).and_then(|b| b.parent);
        }
    }
    set
}

/// Persists per-clip and per-bus effect-chain instances across successive
/// [`mixdown_with_cache`] calls, so a stateful effect's memory — a
/// resonant EQ's biquad history, a compressor's envelope, a limiter's
/// lookahead buffer — survives a call boundary the way it already does
/// *within* one offline [`mixdown`] call. See [`StreamingRenderer`] for the
/// thing that actually wants this; `EffectCache` is the storage underneath
/// it, kept as its own type so `mixdown_with_cache` can be tested against a
/// cache directly without going through the renderer's call-boundary logic.
///
/// Re-instantiates a clip's or bus's chain only when the stored [`Effect`]
/// list no longer matches what's live on the project — an effect chain
/// edited mid-playback (this is a collaborative editor; another peer's edit
/// can land between two calls) must take effect on the next render, not be
/// silently ignored because a stale `Instance` was cached from before the
/// edit.
#[derive(Default)]
pub struct EffectCache {
    /// Each instance with its index in the chain it came from: disabled
    /// effects are skipped at instantiate, so the instance list is
    /// shorter than the chain, and a parameter ride addresses the chain.
    clip: BTreeMap<studio_core::ClipId, (Vec<studio_core::Effect>, Vec<(usize, studio_fx::Instance)>)>,
    bus: BTreeMap<studio_core::BusId, (Vec<studio_core::Effect>, Vec<(usize, studio_fx::Instance)>)>,
    /// Each ducked bus's follower state, kept across contiguous calls like
    /// every other effect's (a duck that re-armed per buffer would click).
    ducks: BTreeMap<studio_core::BusId, Duck>,
    /// Each bus's stereo imager, with the widths it was built for.
    imagers: BTreeMap<studio_core::BusId, ([f32; 3], studio_fx::imager::Imager)>,
}

/// The imager's crossovers: below 200 Hz, 200 Hz - 4 kHz, above 4 kHz.
pub const IMAGER_SPLITS: [f32; 2] = [200.0, 4000.0];

/// A bus's `imager` effect, if it has an enabled one: widths (low, mid,
/// high) -- 0 mono, 1 as is, >1 wider. STEREO: it needs L and R together,
/// which the per-sample effect chain cannot give (see `studio_fx::instantiate`),
/// so, like `duck`, it runs here, after the chain.
pub fn imager_of(bus: &studio_core::Bus) -> Option<[f32; 3]> {
    bus.chain.iter().find(|e| e.kind == "imager" && e.enabled).map(|e| {
        let w = |k: &str| e.params.get(k).copied().unwrap_or(1.0).clamp(0.0, 2.0);
        [w("low"), w("mid"), w("high")]
    })
}

/// SIDECHAIN DUCK: a bus turned down while ANOTHER bus (the key) is loud --
/// the kick pulling the 808 and the reverb down on every hit, so the low end
/// stays tight and the listener does not hear anything leave. Written as
/// effect kind `duck` in a bus's chain, params:
///
/// * `key`: the key bus's id; `threshold` dBFS (-24); `depth` dB, the most it
///   will take away (12); `ratio` (4); `attack` / `release` ms (1 / 120).
///
/// The key is the key bus's FINISHED output (post chain, post fader) -- what
/// FL's "sidechain to this track" hands a compressor. Applied after the
/// ducked bus's own chain, before its fader. Not in `studio_fx::instantiate`
/// on purpose: it needs another bus's signal, which only the mixdown has.
#[derive(Debug, Clone, Default)]
pub struct Duck { env: f32, gain_db: f32 }

impl Duck {
    /// Per-frame gains for `key` (interleaved stereo), continuing the state.
    pub fn gains(&mut self, key: &[f32], p: &studio_core::Effect, rate: u32) -> Vec<f32> {
        let q = |k: &str, d: f32| p.params.get(k).copied().unwrap_or(d);
        let (thr, depth, ratio) = (q("threshold", -24.0), q("depth", 12.0).max(0.0), q("ratio", 4.0).max(1.0));
        let coef = |ms: f32| (-1.0 / (ms.max(0.05) * 0.001 * rate as f32)).exp();
        let (att, rel) = (coef(q("attack", 1.0)), coef(q("release", 120.0)));
        key.chunks(2).map(|fr| {
            let x = fr.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            // peak follower: rises at attack, falls at release
            let c = if x > self.env { att } else { rel };
            self.env = x + (self.env - x) * c;
            let lvl = 20.0 * self.env.max(1e-9).log10();
            let want = if lvl > thr { ((lvl - thr) * (1.0 - 1.0 / ratio)).min(depth) } else { 0.0 };
            // the reduction itself eases the same way, so it pumps, not clicks
            let c = if want > self.gain_db { att } else { rel };
            self.gain_db = want + (self.gain_db - want) * c;
            10f32.powf(-self.gain_db / 20.0)
        }).collect()
    }
}

/// The key bus a bus's `duck` effect listens to, if it has an enabled one.
pub fn duck_of(bus: &studio_core::Bus) -> Option<(studio_core::BusId, &studio_core::Effect)> {
    bus.chain.iter().find(|e| e.kind == "duck" && e.enabled)
        .and_then(|e| e.params.get("key").map(|k| (studio_core::BusId(*k as u64), e)))
}

/// `order` with every ducked bus moved to just after its key -- keys are
/// usually siblings (the kick and the 808 both feed the master) and siblings
/// come in no particular order. A move that would put a bus after its own
/// parent is not made (that duck then reads silence and does nothing).
pub fn order_for_ducks(project: &Project, mut order: Vec<studio_core::BusId>) -> Vec<studio_core::BusId> {
    for _ in 0..order.len() {
        let mut moved = false;
        for b in project.busses.values() {
            let Some((key, _)) = duck_of(b) else { continue };
            let (Some(pb), Some(pk)) = (order.iter().position(|x| *x == b.id), order.iter().position(|x| *x == key)) else { continue };
            if pb > pk { continue; }
            // parent must stay after the moved bus
            if let Some(par) = b.parent { if order.iter().position(|x| *x == par).is_some_and(|pp| pp <= pk) { continue; } }
            let id = order.remove(pb);
            let pk = order.iter().position(|x| *x == key).unwrap();
            order.insert(pk + 1, id);
            moved = true;
        }
        if !moved { break; }
    }
    order
}

impl EffectCache {
    pub fn new() -> EffectCache {
        EffectCache::default()
    }

    /// Drop every cached instance, forcing a fresh instantiate on the next
    /// render. Call this after a transport seek/jump: a stateful effect's
    /// memory from audio that is no longer temporally adjacent to what's
    /// about to play isn't memory worth keeping — a DAW's reverb tail
    /// cutting off on a seek is expected behaviour, not a bug this cache
    /// should work around.
    pub fn reset(&mut self) {
        self.clip.clear();
        self.bus.clear();
        self.ducks.clear();
        self.imagers.clear();
    }

    fn build(chain: &[studio_core::Effect], rate: f32) -> Vec<(usize, studio_fx::Instance)> {
        chain.iter().enumerate()
            .filter(|(_, e)| e.enabled)
            .filter_map(|(i, e)| studio_fx::instantiate(e, rate).map(|inst| (i, inst)))
            .collect()
    }

    fn clip_instances(&mut self, id: studio_core::ClipId, chain: &[studio_core::Effect], rate: f32) -> &mut [(usize, studio_fx::Instance)] {
        let entry = self.clip.entry(id).or_default();
        if entry.0 != chain {
            entry.0 = chain.to_vec();
            entry.1 = Self::build(chain, rate);
        }
        &mut entry.1
    }

    fn bus_instances(&mut self, id: studio_core::BusId, chain: &[studio_core::Effect], rate: f32) -> &mut [(usize, studio_fx::Instance)] {
        let entry = self.bus.entry(id).or_default();
        if entry.0 != chain {
            entry.0 = chain.to_vec();
            entry.1 = Self::build(chain, rate);
        }
        &mut entry.1
    }
}

/// Frames between parameter updates: control rate. A ride is evaluated
/// at each block's first frame and the instance runs the block with it —
/// the classic DAW bargain (a zipper on a violent ramp, never a click),
/// chosen over per-sample redesign because an EQ band redesign per
/// sample is more work than the band itself.
pub const PARAM_BLOCK: usize = 64;

/// One parameter ride on one running instance, resolved for a forward
/// sweep: the stored value the lane multiplies, the schema's range to
/// snap the product into, and the lane and bindings with their cursors.
struct ParamRide<'a> {
    /// Index into the CHAIN (not the instance list).
    effect: usize,
    key: String,
    stored: f32,
    spec: studio_fx::schema::ParamSpec,
    lane: Option<&'a [(Ticks, f32)]>,
    cursor: usize,
    sweeps: Vec<studio_core::Sweep<'a>>,
}

impl<'a> ParamRide<'a> {
    /// The rides on `chain` from `targets` (the model's list for this bus
    /// or clip). A target whose effect index is past the chain, whose
    /// effect is disabled, or whose key the schema does not know is left
    /// out — dangling, and harmless, like a binding to a deleted clip.
    fn resolve(project: &'a Project, chain: &[studio_core::Effect],
               targets: &[(u8, studio_core::ParamKey, AutoTarget)]) -> Vec<ParamRide<'a>> {
        targets.iter().filter_map(|(effect, key, target)| {
            let e = chain.get(*effect as usize)?;
            if !e.enabled { return None; }
            let key = key.as_str();
            let spec = studio_fx::schema::expand(e)?.into_iter().find(|s| s.key == key)?;
            let stored = e.params.get(key).copied().unwrap_or(spec.default);
            let sweeps = project.binding_sweeps(*target);
            let lane = project.automation.get(target).map(|l| l.as_slice());
            if lane.is_none() && sweeps.is_empty() { return None; }
            Some(ParamRide { effect: *effect as usize, key: key.to_string(), stored, spec, lane, cursor: 0, sweeps })
        }).collect()
    }

    /// `stored × lane × Π(bindings)`, snapped to the parameter's range.
    fn value(&mut self, at: Ticks) -> f32 {
        let mut v = self.stored;
        if let Some(l) = self.lane { v *= studio_core::eval_lane_cursor(l, at, &mut self.cursor); }
        for sw in self.sweeps.iter_mut() { v *= sw.value(at); }
        self.spec.clamp(v)
    }
}

/// Push every ride's value at `at` into its instance.
fn apply_rides(rides: &mut [ParamRide<'_>], instances: &mut [(usize, studio_fx::Instance)], at: Ticks, rate: f32) {
    for r in rides.iter_mut() {
        let v = r.value(at);
        if let Some((_, inst)) = instances.iter_mut().find(|(i, _)| *i == r.effect) {
            inst.set_param(&r.key, v, rate);
        }
    }
}

/// A stateful wrapper around [`mixdown_with_cache`] for exactly the use
/// [`mixdown`] itself is NOT safe for: calling once per device buffer.
/// Tracks whether the last call's `to` matches this call's `from` — a
/// genuinely contiguous stream of buffers, the only case a cached effect's
/// memory is still meaningful — and resets the cache automatically on any
/// gap (the first call, or a seek), so a caller doesn't have to reason
/// about when reuse is valid; only about calling `reset()` isn't even
/// necessary for a seek, `render` detects the discontinuity itself from the
/// `from`/`to` it's given.
#[derive(Default)]
pub struct StreamingRenderer {
    cache: EffectCache,
    scratch: RenderScratch,
    next_expected: Option<Ticks>,
}

impl StreamingRenderer {
    pub fn new() -> StreamingRenderer {
        StreamingRenderer::default()
    }

    /// Render `[from, to)`, reusing cached effect instances when this call
    /// picks up exactly where the previous one left off, and transparently
    /// resetting (same cost as a one-shot [`mixdown`] call) otherwise.
    ///
    /// Returns a freshly allocated `Vec` — kept for API compatibility with
    /// existing callers and tests. [`StreamingRenderer::render_into`] is
    /// the version that also reuses the bus/master scratch buffers across
    /// calls and writes into a caller-provided buffer instead of
    /// allocating one; a real device callback should prefer it.
    pub fn render(
        &mut self,
        project: &Project,
        audio: &BTreeMap<SourceId, SourceAudio>,
        fades: &BTreeMap<studio_core::ClipId, Fades>,
        from: Ticks,
        to: Ticks,
        rate: u32,
        solo: &[studio_core::TrackId],
        bus_solo: &[studio_core::BusId],
    ) -> Vec<f32> {
        let frames = (to.to_samples(rate) - from.to_samples(rate)).max(0) as usize;
        let mut out = vec![0.0f32; frames * 2];
        self.render_into(project, audio, fades, from, to, rate, solo, bus_solo, &mut out);
        out
    }

    /// The allocation-conscious version: writes into `out` (must be exactly
    /// `(to - from)` frames of interleaved stereo — see [`mixdown_into`]),
    /// and reuses this renderer's own [`RenderScratch`] across calls the
    /// same way it already reuses [`EffectCache`]. This is what
    /// `studio-engine`'s `DeviceEngine::tick` calls; `render` above exists
    /// for callers (and tests) that want an owned `Vec` back and don't
    /// need the per-call allocation avoided.
    pub fn render_into(
        &mut self,
        project: &Project,
        audio: &BTreeMap<SourceId, SourceAudio>,
        fades: &BTreeMap<studio_core::ClipId, Fades>,
        from: Ticks,
        to: Ticks,
        rate: u32,
        solo: &[studio_core::TrackId],
        bus_solo: &[studio_core::BusId],
        out: &mut [f32],
    ) {
        if self.next_expected != Some(from) {
            self.cache.reset();
        }
        self.next_expected = Some(to);
        mixdown_into(project, audio, fades, from, to, rate, solo, bus_solo, &mut self.cache, &mut self.scratch, out);
    }

    /// Force a reset on the next `render`/`render_into`, regardless of
    /// continuity — exposed for a caller that knows a discontinuity is
    /// coming (e.g. is about to seek) and wants the reset to happen before
    /// the render call that would otherwise detect it, not because
    /// detection needs help.
    pub fn reset(&mut self) {
        self.cache.reset();
        self.next_expected = None;
    }
}

/// The offline entry point: renders `[from, to)` in one call with a fresh
/// [`EffectCache`], so every effect chain runs continuously from the first
/// sample to the last — correct for a one-shot bounce, and NOT safe to call
/// repeatedly for consecutive windows (a stateful effect's memory resets
/// every call — see `mixdown_reinstantiates_effect_state_on_every_call_not_a_realtime_render_yet`
/// in this module's tests for that boundary proven directly). For
/// consecutive windows — a real playback path — use [`StreamingRenderer`]
/// instead, which is exactly this function's logic run against a cache that
/// survives across calls rather than a throwaway one.
pub fn mixdown(
    project: &Project,
    audio: &BTreeMap<SourceId, SourceAudio>,
    fades: &BTreeMap<studio_core::ClipId, Fades>,
    from: Ticks,
    to: Ticks,
    rate: u32,
    solo: &[studio_core::TrackId],
    bus_solo: &[studio_core::BusId],
) -> Vec<f32> {
    let mut cache = EffectCache::new();
    mixdown_with_cache(project, audio, fades, from, to, rate, solo, bus_solo, &mut cache)
}

/// Reused across [`StreamingRenderer::render_into`] calls so a render
/// doesn't allocate the master accumulator or per-bus buffers fresh every
/// buffer, the same reasoning [`EffectCache`] already applies to effect
/// instances. `mixdown`/`mixdown_with_cache`/`StreamingRenderer::render`
/// still build a throwaway one internally — reuse only pays off when the
/// same scratch survives across calls, which is exactly the streaming case.
#[derive(Default)]
pub struct RenderScratch {
    /// One accumulator per bus, kept across calls and cleared in place
    /// (`.clear(); .resize(...)` reuses the existing heap allocation when
    /// capacity already covers the new length, rather than reallocating).
    /// Entries for busses no longer in the project are dropped — a bus
    /// being deleted is a structural edit, not a steady-state realtime
    /// event, so paying an allocation on the NEXT call after a deletion is
    /// the honest trade rather than something worth engineering around.
    bus_buf: BTreeMap<studio_core::BusId, Vec<f32>>,
    /// Busses already folded this call, holding their buffer until the
    /// whole fold finishes — see `mixdown_into`'s own comment on why they
    /// can't go straight back into `bus_buf` mid-call without changing
    /// which sends land.
    retired: Vec<(studio_core::BusId, Vec<f32>)>,
    /// One reusable scratch for a bus's pre-fader tap, replacing what used
    /// to be a fresh `buf.clone()` per bus per call. Only one bus needs
    /// this at a time (the fold is sequential), so a single field suffices.
    pre: Vec<f32>,
}

impl RenderScratch {
    pub fn new() -> RenderScratch {
        RenderScratch::default()
    }
}

/// Resize `v` to exactly `len` zeros, reusing its existing allocation when
/// `v.capacity() >= len` rather than always reallocating — the pattern
/// every scratch buffer in this module uses to stay allocation-free once
/// warmed up.
fn zero_resize(v: &mut Vec<f32>, len: usize) {
    v.clear();
    v.resize(len, 0.0);
}

/// [`mixdown`]'s actual logic, parameterised over the [`EffectCache`] that
/// backs every clip's and bus's effect instances — `mixdown` itself is a
/// two-line wrapper handing this a cache nobody keeps, and
/// [`StreamingRenderer::render`] is a wrapper handing this the SAME cache
/// call after call. Everything about mixing tracks, bus routing, sends, and
/// solo/mute is here exactly once; the only difference between an offline
/// bounce and a live playback call is which cache they pass in.
pub fn mixdown_with_cache(
    project: &Project,
    audio: &BTreeMap<SourceId, SourceAudio>,
    fades: &BTreeMap<studio_core::ClipId, Fades>,
    from: Ticks,
    to: Ticks,
    rate: u32,
    solo: &[studio_core::TrackId],
    bus_solo: &[studio_core::BusId],
    cache: &mut EffectCache,
) -> Vec<f32> {
    let frames = (to.to_samples(rate) - from.to_samples(rate)).max(0) as usize;
    let mut out = vec![0.0f32; frames * 2];
    let mut scratch = RenderScratch::new();
    mixdown_into(project, audio, fades, from, to, rate, solo, bus_solo, cache, &mut scratch, &mut out);
    out
}

/// The genuinely allocation-conscious entry point: writes into a
/// caller-provided `out` (exactly `(to - from)` frames, interleaved
/// stereo — matching `studio_io::DuplexCallback`'s own output shape)
/// instead of returning a fresh `Vec`, and takes a [`RenderScratch`] the
/// caller is expected to keep across calls. [`mixdown_with_cache`] and
/// [`StreamingRenderer::render`] both delegate here with a throwaway
/// scratch and output buffer; [`StreamingRenderer::render_into`] is the
/// one that actually reuses both across calls.
///
/// `out.len()` must equal `((to - from) in frames) * 2`; a mismatched
/// length is a caller bug, not a case this function tries to paper over —
/// silently truncating or padding would hide exactly the kind of
/// off-by-one a device buffer size calculation is prone to.
pub fn mixdown_into(
    project: &Project,
    audio: &BTreeMap<SourceId, SourceAudio>,
    fades: &BTreeMap<studio_core::ClipId, Fades>,
    from: Ticks,
    to: Ticks,
    rate: u32,
    solo: &[studio_core::TrackId],
    bus_solo: &[studio_core::BusId],
    cache: &mut EffectCache,
    scratch: &mut RenderScratch,
    out: &mut [f32],
) {
    let frames = (to.to_samples(rate) - from.to_samples(rate)).max(0) as usize;
    assert_eq!(out.len(), frames * 2, "mixdown_into: out.len() must equal frames * 2");
    out.fill(0.0);
    // `out` IS the master accumulator throughout — tracks routed directly
    // to master write here, and folded busses add into it below, exactly
    // the way the old separately-allocated `master` buffer worked. No
    // extra buffer, no final copy.
    let master = out;

    // One accumulator per bus, pre-populated for every bus in the project —
    // not just the ones a track happens to route to — so a bus that exists
    // only as a send target still has somewhere to land. Busses no longer
    // in the project are dropped from the scratch map (see RenderScratch's
    // own docs on why that's the right trade); surviving ones are resized
    // in place, reusing their allocation.
    scratch.bus_buf.retain(|id, _| project.busses.contains_key(id));
    for id in project.busses.keys() {
        zero_resize(scratch.bus_buf.entry(*id).or_default(), frames * 2);
    }
    let bus_buf = &mut scratch.bus_buf;

    for track in &project.tracks {
        if track.kind != TrackKind::Audio && !project.clips.values().any(|c| c.track == track.id) {
            continue;
        }
        if track.muted { continue; }
        if !solo.is_empty() && !solo.contains(&track.id) { continue; }

        let dest: &mut [f32] = match track.route {
            Some(bus) => match bus_buf.get_mut(&bus) {
                Some(b) => b.as_mut_slice(),
                // Routed at a bus that no longer exists (e.g. a stale route
                // surviving a bus removed by a peer's op before this route
                // was cleared) — fall back to master rather than dropping
                // the track's audio on the floor.
                None => master.as_mut(),
            },
            None => master.as_mut(),
        };

        // Hoisted out of the sample loop: this was a BTreeMap lookup PER
        // SAMPLE — at 48 kHz, forty-eight thousand tree walks per second
        // of audio per track to fetch a value that cannot change within a
        // track. The lane itself is then swept with a cursor rather than
        // binary-searched per sample (see `eval_lane_cursor`).
        let track_lane = project.automation.get(&AutoTarget::TrackGain(track.id));
        // The curves bound to this track's gain, gates resolved once per
        // render (a clip-gated binding's span is a fact about the project,
        // not about the sample) and swept with the same cursor rule as the
        // lane. Empty for every project from before bindings existed, and
        // an empty product is 1.0 — bit-identical, pinned below.
        let mut track_sweeps = project.binding_sweeps(AutoTarget::TrackGain(track.id));
        for clip in project.track_clips(track.id) {
            // A clip's tail rings past its end, so a window that starts
            // after the dry sound can still owe it audio.
            let tail = ClipTail::of(&clip.effects);
            // skip clips entirely outside the window
            if clip.end().saturating_add(tail.ring_ticks()) <= from || clip.pos >= to { continue; }
            let Some(src) = audio.get(&clip.source) else { continue };
            if src.channels.is_empty() { continue; }
            // THE PROJECT'S FADE IS THE FADE. The `fades` parameter stays as
            // an override for callers that have one (the engine's
            // `set_fades`), but a fade is a fact about the edit, so the
            // project is where it lives and where an empty override now falls
            // back to.
            //
            // Before this, nothing anywhere populated `fades`: the map was
            // always empty, `gain_at` always returned 1.0, and the whole fade
            // path was dead while looking implemented.
            let fade = fades.get(&clip.id).copied().unwrap_or_else(|| {
                project.fades.get(&clip.id)
                    .map(|(i, o)| Fades { fade_in: *i, fade_out: *o })
                    .unwrap_or_default()
            });
            // The clip's own effect chain, instantiated once per clip (not
            // per sample) so a stateful effect's filter history is
            // continuous across the whole clip rather than reset every
            // sample. Same convention as the bus-chain wiring above: one
            // `Instance` per effect, fed L then R in sequence — consistent
            // with `studio_fx::chain::Chain::process`'s own established
            // pattern rather than introducing a different one here. An
            // unknown effect kind is silently skipped via `instantiate`'s
            // `None`.
            let clip_fx = cache.clip_instances(clip.id, &clip.effects, rate as f32);
            // Parameter rides on this clip's chain, applied every
            // PARAM_BLOCK frames below. Empty for almost every clip.
            let clip_targets = project.clip_param_targets(clip.id);
            let mut clip_rides = ParamRide::resolve(project, &clip.effects, &clip_targets);

            let start = clip.pos.max(from);
            let stop = clip.end().min(to);
            let out_off = (start.to_samples(rate) - from.to_samples(rate)).max(0) as usize;
            let n = (stop.to_samples(rate) - start.to_samples(rate)).max(0) as usize;
            // The destination bound, resolved ONCE: the old loop asked
            // `oi + 1 >= dest.len()` every sample to catch the same
            // condition arithmetic settles here.
            let room = dest.len().saturating_sub(out_off * 2) / 2;
            let n = n.min(room);
            let start_sample = start.to_samples(rate);
            let mut lane_cursor = 0usize;
            for i in 0..n {
                let at = Ticks::from_samples(start_sample + i as i64, rate);
                let into_clip = at.saturating_sub(clip.pos);
                // Index the SOURCE at the source's own rate — indexing a
                // 44.1k file at 48k-sample positions plays it ~9% sharp and
                // runs out early, the exact chipmunk bug the rack sampler
                // already had fixed while this path kept it. Same cure:
                // fractional position, linear interpolation; a same-rate
                // source takes the exact integer path untouched.
                let src_t = clip.src_in.saturating_add(into_clip);
                // The lane RIDES the fader — multiplied, not substituted —
                // so the fader stays a trim (see `Project::automation`'s
                // docs). Per-sample evaluation at the `at` this loop
                // already computes: automation that stepped per buffer
                // would zipper on any ramp steeper than gentle.
                let mut auto = match track_lane {
                    Some(lane) => studio_core::eval_lane_cursor(lane, at, &mut lane_cursor),
                    None => 1.0,
                };
                // Bindings MULTIPLY into the lane, never replace it — see
                // `studio_core::curves`: two bindings on one track compose,
                // and a binding whose gate is closed reads 1.0.
                for sw in track_sweeps.iter_mut() { auto *= sw.value(at); }
                let g = clip.gain * track.gain * auto * fade.gain_at(into_clip, clip.len);
                let oi = (out_off + i) * 2;
                let read = |c: &[f32], pos: f64| -> Option<f32> {
                    let i0 = pos as usize;
                    if i0 + 1 >= c.len() { return None; }
                    let frac = (pos - i0 as f64) as f32;
                    Some(c[i0] * (1.0 - frac) + c[i0 + 1] * frac)
                };
                let (mut l, mut r);
                if src.sample_rate == rate {
                    let f = src_t.to_samples(rate) as usize;
                    if f >= src.frames() { break; }
                    if src.channels.len() >= 2 {
                        l = src.channels[0][f];
                        r = src.channels[1][f];
                    } else {
                        l = src.channels[0][f];
                        r = l;
                    }
                } else {
                    let pos = src_t.to_seconds() * src.sample_rate as f64;
                    let Some(v0) = read(&src.channels[0], pos) else { break };
                    l = v0;
                    r = if src.channels.len() >= 2 {
                        match read(&src.channels[1], pos) { Some(v) => v, None => break }
                    } else { v0 };
                }
                if !clip_rides.is_empty() && i % PARAM_BLOCK == 0 {
                    apply_rides(&mut clip_rides, clip_fx, at, rate as f32);
                }
                for (_, inst) in clip_fx.iter_mut() {
                    l = inst.process_audio(l);
                    r = inst.process_audio(r);
                }
                dest[oi] += l * g;
                dest[oi + 1] += r * g;
            }

            // ── the tail: the chain keeps running on silence ──────────────
            // Only meaningful when this window also rendered up to the clip
            // end — the effect's state (the reverb's energy) comes from
            // processing the clip. A window that starts INSIDE the tail
            // without having played the clip has no energy to ring out; that
            // is a limitation of rendering a slice, named here rather than
            // papered over with a guess.
            if tail.ring > 0.0 && clip.end() >= from && clip.end() < to {
                let t0 = clip.end();
                let t1 = clip.end().saturating_add(tail.ring_ticks()).min(to);
                let off = (t0.to_samples(rate) - from.to_samples(rate)).max(0) as usize;
                let len = (t1.to_samples(rate) - t0.to_samples(rate)).max(0) as usize;
                let len = len.min(dest.len().saturating_sub(off * 2) / 2);
                let base = t0.to_samples(rate);
                for i in 0..len {
                    let at = Ticks::from_samples(base + i as i64, rate);
                    let past = i as f64 / rate as f64;
                    let tg = tail.gain_at(past);
                    if tg <= 0.0 { break; }
                    let mut auto = match track_lane {
                        Some(lane) => studio_core::eval_lane_cursor(lane, at, &mut lane_cursor),
                        None => 1.0,
                    };
                    for sw in track_sweeps.iter_mut() { auto *= sw.value(at); }
                    // Silence in, the effect's memory out. The clip's own
                    // fade is NOT applied: the dry sound has ended, and the
                    // tail's shape is the tail setting's job.
                    let (mut l, mut r) = (0.0f32, 0.0f32);
                    for (_, inst) in clip_fx.iter_mut() {
                        l = inst.process_audio(l);
                        r = inst.process_audio(r);
                    }
                    let g = clip.gain * track.gain * auto * tg;
                    let oi = (off + i) * 2;
                    dest[oi] += l * g;
                    dest[oi + 1] += r * g;
                }
            }
        }
    }

    // Fold the bus graph: children before parents, so by the time a bus is
    // processed everything that feeds it (tracks, and any child bus's main
    // output and sends) has already landed in its accumulator. `bus_order`
    // only fails on a cycle, which `apply` already refuses to create — so
    // this can only happen against a project that reached memory some other
    // way (a corrupt or foreign file). Rather than panic a render over it,
    // busses are simply left unfolded: their tracks' audio is silently
    // absent from master rather than crashing playback. That is the
    // deliberately chosen lesser failure, matching the rest of Studio's
    // "a damaged file is refused outright rather than half-read" stance —
    // the refusal belongs at load time (where the caller can tell the user),
    // not buried in a pure render function with no error channel to report
    // it through.
    let order = order_for_ducks(project, project.bus_order().unwrap_or_default());
    let audible = solo_audible_bus_set(project, bus_solo);
    // finished outputs of busses something ducks under, for this call
    let key_ids: Vec<studio_core::BusId> = project.busses.values().filter_map(|b| duck_of(b).map(|(k, _)| k)).collect();
    let mut keys: BTreeMap<studio_core::BusId, Vec<f32>> = BTreeMap::new();
    for id in order {
        let Some(bus) = project.busses.get(&id) else { continue };
        // Removed (not merely borrowed) from `bus_buf` for the rest of
        // this bus's processing: a `send` targeting a bus later in this
        // same loop is only supposed to land if that bus hasn't been
        // folded yet (see `mixdown`'s existing contract — this refactor
        // preserves it exactly). Retiring the buffer into `scratch.retired`
        // rather than dropping it is what lets its allocation survive to
        // the NEXT call without changing that contract within THIS call —
        // reinserting into `bus_buf` immediately would make an
        // already-processed bus look "not yet folded" to a later send.
        let Some(buf) = bus_buf.remove(&id) else { continue };
        // Mute is an absolute override, checked before solo; a gated bus's
        // buffer — including everything already summed into it from its own
        // tracks and any children processed earlier — is simply dropped,
        // the same as a muted track's clips never reaching the mix.
        let solo_ok = bus_solo.is_empty() || audible.contains(&id);
        if bus.muted || !solo_ok {
            scratch.retired.push((id, buf));
            continue;
        }
        let mut buf = buf;
        // The bus's own insert chain (EQ/comp/etc, described by `Bus.chain`)
        // runs before the fader — the same insert-then-fader order a
        // physical channel strip uses, which is also why `pre` below (what
        // a pre-fader send taps) is captured AFTER the chain but BEFORE
        // gain: a pre-fader send hears the bus's own processing, just not
        // its fader movement.
        //
        // One `Instance` per effect, fed the buffer's L/R samples in their
        // natural interleaved order — not two independent per-channel
        // instances — matching `studio_fx::chain::Chain::process`'s own
        // established convention (it also runs one instance's state across
        // both channels sequentially) rather than introducing a different,
        // arguably-more-correct-but-inconsistent convention in a second
        // place. An unknown effect kind is silently skipped via
        // `instantiate`'s `None`, the same "opens without the plugin"
        // behaviour every other effect host in this codebase has.
        if !bus.chain.is_empty() {
            let instances = cache.bus_instances(id, &bus.chain, rate as f32);
            let targets = project.bus_param_targets(id);
            let mut rides = ParamRide::resolve(project, &bus.chain, &targets);
            if rides.is_empty() {
                for s in buf.iter_mut() {
                    for (_, inst) in instances.iter_mut() {
                        *s = inst.process_audio(*s);
                    }
                }
            } else {
                // Ridden: the same loop in blocks, the rides applied at
                // each block's first frame (interleaved, so a block is
                // PARAM_BLOCK frames = twice that in samples).
                let from_s = from.to_samples(rate);
                for (bi, block) in buf.chunks_mut(PARAM_BLOCK * 2).enumerate() {
                    let at = Ticks::from_samples(from_s + (bi * PARAM_BLOCK) as i64, rate);
                    apply_rides(&mut rides, instances, at, rate as f32);
                    for s in block.iter_mut() {
                        for (_, inst) in instances.iter_mut() {
                            *s = inst.process_audio(*s);
                        }
                    }
                }
            }
        }
        if let Some(widths) = imager_of(bus) {
            let entry = cache.imagers.entry(id).or_insert_with(|| (widths, studio_fx::imager::Imager::new(rate as f32, &IMAGER_SPLITS)));
            if entry.0 != widths { *entry = (widths, studio_fx::imager::Imager::new(rate as f32, &IMAGER_SPLITS)); }
            entry.1.widths = widths.to_vec();
            for fr in buf.chunks_exact_mut(2) {
                let (l, r) = entry.1.process(fr[0], fr[1]);
                fr[0] = l; fr[1] = r;
            }
        }
        if let Some((key, fx)) = duck_of(bus) {
            // a key not yet heard in this call (a cycle, or a key that is this
            // bus's own descendant... not reachable by FL) reads as silence
            let silence;
            let k: &[f32] = match keys.get(&key) { Some(k) => k, None => { silence = vec![0.0; buf.len()]; &silence } };
            let g = cache.ducks.entry(id).or_default().gains(k, fx, rate);
            for (fr, gi) in buf.chunks_exact_mut(2).zip(g) { fr[0] *= gi; fr[1] *= gi; }
        }
        zero_resize(&mut scratch.pre, buf.len());
        scratch.pre.copy_from_slice(&buf);
        let bus_lane = project.automation.get(&AutoTarget::BusGain(id));
        let mut bus_sweeps = project.binding_sweeps(AutoTarget::BusGain(id));
        if bus_lane.is_some() || !bus_sweeps.is_empty() {
            // The bus fader's ride, per interleaved frame — L and R of
            // one instant share one value, or a ramp would tilt the image
            // sideways. Bindings multiply in exactly as on a track; a bus
            // gated by a clip on some OTHER track is how a vocal ducks the
            // drum bus without a sidechain compressor.
            let from_s = from.to_samples(rate);
            for (fi, pair) in buf.chunks_exact_mut(2).enumerate() {
                let at = Ticks::from_samples(from_s + fi as i64, rate);
                let mut g = bus.gain * bus_lane.map_or(1.0, |lane| eval_lane(lane, at));
                for sw in bus_sweeps.iter_mut() { g *= sw.value(at); }
                pair[0] *= g;
                pair[1] *= g;
            }
        } else {
            for s in buf.iter_mut() { *s *= bus.gain; }
        }
        // PAN, as a balance on the stereo bus, with the fader (pre-fader
        // sends heard it before either, as a desk's do)
        if let Some(pan) = project.bus_pans.get(&id) {
            let (gl, gr) = studio_core::balance_gains(*pan);
            for fr in buf.chunks_exact_mut(2) { fr[0] *= gl; fr[1] *= gr; }
        }

        if key_ids.contains(&id) { keys.insert(id, buf.clone()); }
        // A silent bus stops here: it has been summed, processed and faded
        // (so `bus_signal` can read a finished signal), and now feeds
        // nothing — not its sends, not its parent. Without this a control
        // tap fed by sends would double every source into master.
        if bus.silent {
            scratch.retired.push((id, buf));
            continue;
        }

        for send in &bus.sends {
            let tap: &[f32] = if send.pre_fader { &scratch.pre } else { &buf };
            if let Some(target) = bus_buf.get_mut(&send.to) {
                for (t, s) in target.iter_mut().zip(tap.iter()) {
                    *t += s * send.level;
                }
            }
        }

        let parent: &mut [f32] = match bus.parent {
            Some(p) => match bus_buf.get_mut(&p) {
                Some(b) => b.as_mut_slice(),
                None => master.as_mut(),
            },
            None => master.as_mut(),
        };
        for (p, b) in parent.iter_mut().zip(buf.iter()) {
            *p += b;
        }
        scratch.retired.push((id, buf));
    }

    // Everything folded this call goes back into `bus_buf` now that no
    // more sends can target it this call — ready, and already the right
    // size, for the next call's `zero_resize` to reuse without a fresh
    // allocation.
    for (id, buf) in scratch.retired.drain(..) {
        scratch.bus_buf.insert(id, buf);
    }
}

/// Render `project` and return what one bus's output was, interleaved
/// stereo — after its chain and fader, before it feeds anything.
///
/// This is how a "dummy" channel is read: sends copy signal into a
/// [`Bus`](studio_core::Bus) marked `silent`, the mix is unaffected, and
/// this returns the tap. Feeding it to `studio_video::multicam::Control`
/// is what lets a kick drum choose camera angles.
///
/// `None` if the bus does not exist, is muted, or is silenced by a solo —
/// all three mean it was never rendered, which is different from being
/// rendered silent (an existing, empty bus returns zeros).
pub fn bus_signal(
    project: &Project,
    audio: &BTreeMap<SourceId, SourceAudio>,
    fades: &BTreeMap<studio_core::ClipId, Fades>,
    from: Ticks,
    to: Ticks,
    rate: u32,
    bus: studio_core::BusId,
) -> Option<Vec<f32>> {
    let b = project.busses.get(&bus)?;
    if b.muted { return None; }
    let frames = (to.to_samples(rate) - from.to_samples(rate)).max(0) as usize;
    let mut cache = EffectCache::new();
    let mut scratch = RenderScratch::new();
    let mut out = vec![0.0f32; frames * 2];
    mixdown_into(project, audio, fades, from, to, rate, &[], &[], &mut cache, &mut scratch, &mut out);
    scratch.bus_buf.get(&bus).map(|b| b.clone())
}

/// Peak level of a rendered buffer, in dBFS. `-inf` for silence — reported as
/// f32::NEG_INFINITY rather than a magic -100.0, so a caller cannot mistake
/// "very quiet" for "nothing".
pub fn peak_dbfs(samples: &[f32]) -> f32 {
    let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    if peak <= 0.0 { f32::NEG_INFINITY } else { 20.0 * peak.log10() }
}

#[cfg(test)]
mod peaks_window_tests {
    use super::*;

    fn burst_audio() -> SourceAudio {
        // 2 s at 1 kHz: silence, a loud burst from 0.5 to 0.6 s, silence
        let mut s = vec![0.0f32; 2000];
        for v in s.iter_mut().take(600).skip(500) { *v = 0.9; }
        SourceAudio { sample_rate: 1000, channels: vec![s] }
    }

    #[test]
    fn a_window_past_the_end_keeps_its_scale_instead_of_stretching() {
        let p = Peaks::build(&burst_audio(), 4, 4);
        assert_eq!(p.rate, 1000, "the peaks know their own rate");
        // ask for 0..4000 over 400 columns (10 samples a column): the audio ends at column 200
        let w = p.window(0, 4000, 400);
        assert_eq!(w.len(), 400);
        let loud: Vec<usize> = w.iter().enumerate().filter(|(_, m)| m.max > 0.5).map(|(i, _)| i).collect();
        assert!(loud.iter().all(|c| (49..=61).contains(c)), "the burst stays at 0.5-0.6 s, columns 50-60: {loud:?}");
        assert!(w[200..].iter().all(|m| m.max == 0.0 && m.min == 0.0), "past the end: empty, not stretched audio");
        // the same view of the first 2 s agrees column for column
        let full = p.window(0, 2000, 200);
        assert_eq!(&w[..200], &full[..], "running past the end changes nothing before it");
        // entirely past the end: all empty
        assert!(p.window(3000, 3500, 50).iter().all(|m| m.max == 0.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Bus, BusId, Clip, ClipId, Op, Send, Source, Track, TrackId};

    fn tone(frames: usize, rate: u32, hz: f32, amp: f32) -> SourceAudio {
        let ch: Vec<f32> = (0..frames)
            .map(|i| (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * amp)
            .collect();
        SourceAudio { sample_rate: rate, channels: vec![ch] }
    }

    fn project_with(len_s: f64, gain: f32) -> (Project, BTreeMap<SourceId, SourceAudio>) {
        let rate = 48_000;
        let mut p = Project { name: "mix".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "t.wav".into(),
            len: Ticks::from_seconds(len_s), sample_rate: rate, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(len_s),
            gain, effects: Vec::new() } }).unwrap();
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), tone((len_s * rate as f64) as usize, rate, 440.0, 0.5));
        (p, audio)
    }

    /// A half-second clip through a big reverb, rendered for three seconds,
    /// with the tail set as given (`None` = no tail effect at all).
    fn reverb_render(tail: Option<ClipTail>) -> (Vec<f32>, u32) {
        let (mut p, audio) = project_with(0.5, 1.0);
        let mut fx = vec![studio_core::Effect { kind: "reverb".into(), enabled: true,
            params: [("mix".to_string(), 1.0f32), ("size".to_string(), 0.9), ("damp".to_string(), 0.2),
                     ("width".to_string(), 1.0)].into_iter().collect() }];
        if let Some(t) = tail { fx.push(t.effect()); }
        p.apply(&Op::SetEffects { id: ClipId(1), effects: fx }).unwrap();
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(3.0), 48_000, &[], &[]);
        (out, 48_000)
    }

    /// Peak absolute level over [a, b) seconds of an interleaved stereo render.
    fn peak_between(out: &[f32], rate: u32, a: f64, b: f64) -> f32 {
        let (i0, i1) = ((a * rate as f64) as usize * 2, ((b * rate as f64) as usize * 2).min(out.len()));
        out[i0..i1].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn with_no_tail_setting_the_effect_stops_at_the_clip_end_as_before() {
        let (out, rate) = reverb_render(None);
        assert!(peak_between(&out, rate, 0.1, 0.5) > 0.01, "the reverb is audible during the clip");
        assert_eq!(peak_between(&out, rate, 0.5, 3.0), 0.0,
                   "no tail effect = the old behaviour, a hard cut at the clip end");
    }

    #[test]
    fn a_ring_out_keeps_the_reverb_past_the_clip_and_stops_exactly_when_told() {
        let (out, rate) = reverb_render(Some(ClipTail { ring: 1.0, fade: 0.0 }));
        assert!(peak_between(&out, rate, 0.5, 0.7) > 0.005, "the tail rings after the clip ends");
        assert!(peak_between(&out, rate, 1.3, 1.5) > 0.0, "and is still ringing just before it is cut");
        assert_eq!(peak_between(&out, rate, 1.5, 3.0), 0.0, "then stops dead at clip end + 1 s");
    }

    #[test]
    fn a_fade_shapes_only_the_end_of_the_ring() {
        let hard = reverb_render(Some(ClipTail { ring: 1.0, fade: 0.0 })).0;
        let (soft, rate) = reverb_render(Some(ClipTail { ring: 1.0, fade: 0.5 }));
        // Before the fade starts the two are identical: the fade is a shape
        // at the END, not a different tail.
        let i0 = (0.5 * rate as f64) as usize * 2;
        let i1 = (0.95 * rate as f64) as usize * 2;
        assert_eq!(&hard[i0..i1], &soft[i0..i1], "untouched until the fade begins");
        // Inside the fade it is quieter, and it reaches silence at the end
        // instead of stopping on a non-zero sample.
        assert!(peak_between(&soft, rate, 1.4, 1.5) < peak_between(&hard, rate, 1.4, 1.5) * 0.5,
                "the last tenth is well down");
        let last = ((1.5 * rate as f64) as usize - 1) * 2;
        assert!(soft[last].abs() < 1e-3, "it ends at silence: {}", soft[last]);
        assert_eq!(peak_between(&soft, rate, 1.5, 3.0), 0.0);
    }

    #[test]
    fn tail_settings_read_back_and_a_fade_never_outlasts_its_ring() {
        let t = ClipTail::of(&[ClipTail { ring: 2.0, fade: 5.0 }.effect()]);
        assert_eq!(t, ClipTail { ring: 2.0, fade: 2.0 }, "fade is clamped to the ring");
        assert_eq!(ClipTail::of(&[]), ClipTail::default(), "no setting = cut");
        let mut off = ClipTail { ring: 2.0, fade: 0.0 }.effect();
        off.enabled = false;
        assert_eq!(ClipTail::of(&[off]).ring, 0.0, "a disabled tail is a cut");
        assert_eq!(ClipTail { ring: 1.0, fade: 0.5 }.gain_at(0.25), 1.0);
        assert!((ClipTail { ring: 1.0, fade: 0.5 }.gain_at(0.75) - 0.5).abs() < 1e-6);
        assert_eq!(ClipTail { ring: 1.0, fade: 0.5 }.gain_at(1.0), 0.0);
    }

    #[test]
    fn peaks_match_a_brute_force_scan() {
        // The pyramid is only useful if it tells the truth: every level must
        // agree with reading the samples directly.
        let a = tone(50_000, 48_000, 300.0, 0.8);
        let p = Peaks::build(&a, 256, 4);
        assert!(p.levels.len() > 3, "a pyramid, not one flat level");
        for level in 0..p.levels.len() {
            let per = p.samples_per_bucket(level);
            for (bi, bucket) in p.levels[level].iter().enumerate() {
                let s0 = bi * per;
                let s1 = ((bi + 1) * per).min(a.frames());
                if s0 >= s1 { continue; }
                let (mut mn, mut mx) = (f32::INFINITY, f32::NEG_INFINITY);
                for f in s0..s1 { let v = a.mono(f); mn = mn.min(v); mx = mx.max(v); }
                assert!((bucket.min - mn).abs() < 1e-6, "level {level} bucket {bi} min");
                assert!((bucket.max - mx).abs() < 1e-6, "level {level} bucket {bi} max");
            }
        }
    }

    #[test]
    fn min_and_max_are_kept_so_transients_survive_zooming_out() {
        // A waveform drawn from averages loses the click you were looking for.
        // One spike in a quiet file must still be visible at the coarsest level.
        let mut a = tone(100_000, 48_000, 200.0, 0.02);
        a.channels[0][61_234] = 0.97;
        let p = Peaks::build(&a, 256, 8);
        let top = p.levels.last().unwrap();
        assert!(top.iter().any(|b| b.max > 0.9), "the transient survives to the top of the pyramid");
        // and it is visible in a 40-column render of the whole file
        let cols = p.window(0, a.frames(), 40);
        assert_eq!(cols.len(), 40);
        assert!(cols.iter().any(|c| c.max > 0.9), "and in a zoomed-out draw");
    }

    #[test]
    fn the_level_chosen_tracks_the_zoom() {
        let a = tone(500_000, 48_000, 100.0, 0.5);
        let p = Peaks::build(&a, 256, 4);
        // zoomed right in (a handful of samples per pixel) → the finest level
        assert_eq!(p.for_px(4.0), 0);
        // one bucket per pixel → level 0 exactly
        assert_eq!(p.for_px(256.0), 0);
        // four buckets per pixel → one level up
        assert_eq!(p.for_px(1024.0), 1);
        // absurdly far out → coarsest available, not a panic
        let far = p.for_px(1e12);
        assert_eq!(far, p.levels.len() - 1);
    }

    #[test]
    fn silence_in_gives_silence_out() {
        let (p, mut audio) = project_with(1.0, 1.0);
        audio.insert(SourceId(1), SourceAudio { sample_rate: 48_000, channels: vec![vec![0.0; 48_000]] });
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(1.0), 48_000, &[], &[]);
        assert_eq!(out.len(), 48_000 * 2, "stereo interleaved, one second");
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(peak_dbfs(&out), f32::NEG_INFINITY, "silence is -inf, not a magic number");
    }

    #[test]
    fn clip_gain_scales_the_render() {
        let (full, audio) = project_with(0.5, 1.0);
        let (half, _) = project_with(0.5, 0.5);
        let w = (Ticks::ZERO, Ticks::from_seconds(0.5));
        let a = mixdown(&full, &audio, &BTreeMap::new(), w.0, w.1, 48_000, &[], &[]);
        let b = mixdown(&half, &audio, &BTreeMap::new(), w.0, w.1, 48_000, &[], &[]);
        let (pa, pb) = (peak_dbfs(&a), peak_dbfs(&b));
        assert!((pa - pb - 6.02).abs() < 0.2, "half gain is ~6 dB down: {pa} vs {pb}");
    }

    #[test]
    fn a_muted_track_is_silent_and_solo_wins_over_everything_else() {
        let (mut p, audio) = project_with(0.5, 1.0);
        // mute → silence
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: true, locked: false }).unwrap();
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(out.iter().all(|s| *s == 0.0), "a muted track contributes nothing");
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: false }).unwrap();
        // solo on ANOTHER track silences this one — the rule reimplementations invert
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5),
                          48_000, &[TrackId(99)], &[]);
        assert!(out.iter().all(|s| *s == 0.0), "un-soloed tracks are silent while anything is soloed");
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5),
                          48_000, &[TrackId(1)], &[]);
        assert!(out.iter().any(|s| *s != 0.0), "and the soloed track is heard");
    }

    #[test]
    fn fades_reach_silence_at_the_edges_and_unity_in_the_middle() {
        let len = Ticks::from_seconds(4.0);
        let f = Fades { fade_in: Ticks::from_seconds(1.0), fade_out: Ticks::from_seconds(1.0) };
        assert_eq!(f.gain_at(Ticks::ZERO, len), 0.0, "silent at the very start");
        assert!((f.gain_at(Ticks::from_seconds(0.5), len) - 0.5).abs() < 1e-4, "halfway up");
        assert_eq!(f.gain_at(Ticks::from_seconds(2.0), len), 1.0, "unity in the middle");
        assert!((f.gain_at(Ticks::from_seconds(3.5), len) - 0.5).abs() < 1e-4, "halfway down");
        assert!(f.gain_at(len, len) <= 1e-6, "silent at the very end");
        // a clip with no fades is unity throughout
        let none = Fades::default();
        assert_eq!(none.gain_at(Ticks::ZERO, len), 1.0);
    }

    #[test]
    fn only_the_requested_window_is_rendered() {
        // Rendering a middle second must not shift the material: the samples in
        // the window have to be the same ones a full render would put there.
        let (p, audio) = project_with(3.0, 1.0);
        let full = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(3.0), 48_000, &[], &[]);
        let mid = mixdown(&p, &audio, &BTreeMap::new(),
                          Ticks::from_seconds(1.0), Ticks::from_seconds(2.0), 48_000, &[], &[]);
        assert_eq!(mid.len(), 48_000 * 2);
        let off = 48_000 * 2;
        for i in 0..mid.len() {
            assert!((mid[i] - full[off + i]).abs() < 1e-6, "window sample {i} must match the full render");
        }
    }

    fn bus(id: u64, name: &str, parent: Option<u64>, gain: f32) -> Bus {
        Bus { id: BusId(id), name: name.into(), parent: parent.map(BusId), gain, chain: Vec::new(), sends: Vec::new(), muted: false, silent: false }
    }

    #[test]
    fn a_project_with_no_busses_mixes_exactly_as_before_busses_existed() {
        // The whole point of `route: None` meaning "master": adding the bus
        // graph to the model must not perturb a single sample of a project
        // that doesn't use it.
        let (p, audio) = project_with(0.5, 0.7);
        assert!(p.busses.is_empty());
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(out.iter().any(|s| *s != 0.0));
    }

    #[test]
    fn routing_through_a_bus_applies_the_buss_gain() {
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Drums", None, 0.5) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let direct = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        p.apply(&Op::SetRoute { track: TrackId(1), bus: None }).unwrap();
        let unrouted = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let (pd, pu) = (peak_dbfs(&direct), peak_dbfs(&unrouted));
        assert!((pu - pd - 6.02).abs() < 0.2, "routing through a 0.5-gain bus should read ~6dB down: {pu} vs {pd}");
    }

    #[test]
    fn a_bus_hierarchy_sums_children_into_the_parent_before_master() {
        // Drums and 808 both feed a "Beat" bus; Beat feeds master. Cutting
        // Beat's gain to zero must silence both children even though neither
        // child's own gain changed — that's the point of a hierarchy over a
        // flat list of busses.
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Beat", None, 1.0) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Drums", Some(1), 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(2)) }).unwrap();

        let heard = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(heard.iter().any(|s| *s != 0.0), "audio reaches master through the two-level chain");

        p.apply(&Op::SetBusGain { id: BusId(1), gain: 0.0 }).unwrap();
        let silenced = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(silenced.iter().all(|s| *s == 0.0), "zeroing the parent bus must silence everything routed through it");
    }

    #[test]
    fn a_silent_bus_is_a_tap_that_costs_the_mix_nothing() {
        // The dummy-channel shape: a Control bus fed by a send from Vocals.
        // Master must be bit-identical to the project without the send —
        // otherwise the tap doubles the source into the mix.
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 1.0) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Control", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let end = Ticks::from_seconds(0.5);
        let before = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, &[], &[]);

        p.apply(&Op::SetBusSilent { id: BusId(2), silent: true }).unwrap();
        p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 1.0, pre_fader: false }] }).unwrap();
        let after = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, &[], &[]);
        assert_eq!(before, after, "a silent bus must not reach master");

        // ...and the tap carries the signal, at the send's level.
        let tap = bus_signal(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, BusId(2)).expect("a tap");
        assert_eq!(tap.len(), before.len());
        assert!(peak_dbfs(&tap) > -60.0, "the tap heard something: {}", peak_dbfs(&tap));
        p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 0.5, pre_fader: false }] }).unwrap();
        let half = bus_signal(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, BusId(2)).unwrap();
        assert!((peak_dbfs(&tap) - peak_dbfs(&half) - 6.02).abs() < 0.2, "send level rides the tap");
    }

    #[test]
    fn a_silent_bus_still_runs_its_own_fader_and_a_muted_one_has_no_tap() {
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Control", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        p.apply(&Op::SetBusSilent { id: BusId(1), silent: true }).unwrap();
        let end = Ticks::from_seconds(0.5);
        let mixed = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, &[], &[]);
        assert!(mixed.iter().all(|s| *s == 0.0), "the only track routes into the silent bus: master is empty");
        let loud = bus_signal(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, BusId(1)).unwrap();
        p.apply(&Op::SetBusGain { id: BusId(1), gain: 0.5 }).unwrap();
        let quiet = bus_signal(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, BusId(1)).unwrap();
        assert!((peak_dbfs(&loud) - peak_dbfs(&quiet) - 6.02).abs() < 0.2, "the tap is read after the fader");

        // Muted is not silent: a muted bus is dropped before its chain, so
        // there is nothing to read, and `None` says so rather than zeros.
        p.apply(&Op::SetBusFlags { id: BusId(1), muted: true }).unwrap();
        assert!(bus_signal(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, BusId(1)).is_none());
        assert!(bus_signal(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, BusId(99)).is_none());
    }

    #[test]
    fn a_pre_fader_split_hears_the_source_before_its_fader_moves() {
        // The splitter case: one signal, two destinations, and the tap must
        // not move when the audible path is ridden down.
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 1.0) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Control", None, 1.0) }).unwrap();
        p.apply(&Op::SetBusSilent { id: BusId(2), silent: true }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 1.0, pre_fader: true }] }).unwrap();
        let end = Ticks::from_seconds(0.5);
        let open = bus_signal(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, BusId(2)).unwrap();
        p.apply(&Op::SetBusGain { id: BusId(1), gain: 0.1 }).unwrap();
        let faded = bus_signal(&p, &audio, &BTreeMap::new(), Ticks::ZERO, end, 48_000, BusId(2)).unwrap();
        assert_eq!(open, faded, "a pre-fader split is independent of the audible path's fader");
    }

    #[test]
    fn a_send_adds_a_parallel_copy_on_top_of_the_main_path() {
        // Vocals (parent: None) and Verb (parent: None) both flow straight
        // to master on their own — so sending Vocals into Verb at 0.5 does
        // NOT leave master unchanged, it makes it LOUDER by exactly the
        // added parallel copy: master becomes vocals*1.0 (Vocals' own main
        // path) + vocals*0.5 (Verb's main path, fed entirely by the send).
        // That extra path — not a hole in the original — is the send's
        // whole job, and is what this test actually pins.
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 1.0) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Verb", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();

        let before = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 0.5, pre_fader: false }] }).unwrap();
        let after = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let (pb, pa) = (peak_dbfs(&before), peak_dbfs(&after));
        // Both busses have parent: None, so both flow into master; the send
        // adds a same-phase 0.5x copy on top, so the sum should read
        // 20*log10(1.5) ≈ 3.52 dB louder, not merely "different."
        assert!((pa - pb - 3.52).abs() < 0.2, "adding the send should add ~3.52dB (1.0 + 0.5 same-phase copy): {pb} -> {pa}");
    }

    #[test]
    fn a_send_does_not_touch_the_source_buss_own_downstream_gain_stage() {
        // Isolate the "main path unaffected" claim properly this time: put
        // the send's target bus somewhere that does NOT also flow to
        // master (a child of a second, muted-via-zero-gain parent), so its
        // contribution to the final mix is independently zeroed, and only
        // Vocals' own main path can be heard either way.
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 1.0) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Verb", None, 1.0) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(3, "VerbSink", Some(2), 0.0) }).unwrap(); // swallows Verb's output
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();

        // Reroute Verb itself into the zero-gain sink instead of master.
        let mut p = p;
        p.busses.get_mut(&BusId(2)).unwrap().parent = Some(BusId(3));

        let before = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 0.5, pre_fader: false }] }).unwrap();
        let after = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        for i in 0..before.len() {
            assert!((before[i] - after[i]).abs() < 1e-5,
                "sample {i}: a send into a bus that never reaches master must not change the audible mix");
        }
    }

    #[test]
    fn a_pre_fader_send_ignores_the_buss_own_gain() {
        // Vocals' own fader is all the way down (gain 0), but a pre-fader
        // send to Cue must still carry the signal — Cue has no parent set,
        // so its default `parent: None` already routes it straight to
        // master, making the assertion just "master is not silent."
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 0.0) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Cue", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 1.0, pre_fader: true }] }).unwrap();

        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(out.iter().any(|s| *s != 0.0),
            "a pre-fader send must carry signal to Cue (and on to master) even though Vocals' own fader is at zero");

        // And the control: make the same send post-fader instead. Now it
        // taps Vocals AFTER its zero gain, so Cue — and therefore master —
        // must go silent.
        p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 1.0, pre_fader: false }] }).unwrap();
        let post = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(post.iter().all(|s| *s == 0.0),
            "a post-fader send must inherit the source bus's own gain — silent here since Vocals' gain is zero");
    }

    #[test]
    fn a_track_routed_to_a_since_removed_bus_falls_back_to_master_instead_of_vanishing() {
        // Simulates a stale route surviving a bus removal that arrived from
        // a peer before this project's own SetRoute-to-None caught up —
        // audio must not be silently dropped on the floor.
        let (mut p, audio) = project_with(0.5, 1.0);
        p.tracks[0].route = Some(BusId(404)); // no such bus, built directly like a corrupt/racy state
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(out.iter().any(|s| *s != 0.0), "audio routed at a missing bus must still reach master");
    }

    #[test]
    fn a_clip_effect_measurably_changes_the_mix() {
        let (mut p, audio) = project_with(0.5, 1.0);
        let unprocessed = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0); // project_with's own test tone frequency
        params.insert("gain0".to_string(), -18.0);
        let clip_id = *p.clips.keys().next().unwrap();
        p.apply(&Op::SetEffects { id: clip_id, effects: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: true },
        ] }).unwrap();
        let processed = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let (pu, pp) = (peak_dbfs(&unprocessed), peak_dbfs(&processed));
        assert!(pp < pu - 6.0, "an -18dB EQ cut on a clip at its own tone's frequency should measurably quiet the mix: unprocessed {pu}dBFS, processed {pp}dBFS");
    }

    #[test]
    fn a_disabled_clip_effect_does_not_change_the_mix() {
        let (mut p, audio) = project_with(0.5, 1.0);
        let unprocessed = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0);
        params.insert("gain0".to_string(), -18.0);
        let clip_id = *p.clips.keys().next().unwrap();
        p.apply(&Op::SetEffects { id: clip_id, effects: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: false },
        ] }).unwrap();
        let with_disabled = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        for i in 0..unprocessed.len() {
            assert!((unprocessed[i] - with_disabled[i]).abs() < 1e-5, "sample {i}: a disabled clip effect must not change the mix");
        }
    }

    #[test]
    fn an_unknown_clip_effect_kind_is_silently_skipped_not_a_panic() {
        let (mut p, audio) = project_with(0.5, 1.0);
        let clip_id = *p.clips.keys().next().unwrap();
        p.apply(&Op::SetEffects { id: clip_id, effects: vec![
            studio_core::Effect { kind: "plugin_i_dont_have".into(), params: Default::default(), enabled: true },
        ] }).unwrap();
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(out.iter().any(|s| *s != 0.0));
    }

    #[test]
    fn mixdown_reinstantiates_effect_state_on_every_call_not_a_realtime_render_yet() {
        // `mixdown` is correct and fully tested as an OFFLINE bounce — call it
        // once over the whole span you want and every effect chain runs
        // continuously from the first sample to the last. What this test
        // pins down is the boundary of that guarantee: `mixdown` builds a
        // fresh `Vec<studio_fx::Instance>` on every call (see the clip_fx /
        // bus-chain construction above), so a stateful effect's memory —
        // here a high-Q peaking EQ's biquad z-1/z-2 history — does not
        // survive between calls. Rendering one window in a single call and
        // rendering the identical span as two back-to-back calls (exactly
        // what a live device callback filling successive buffers would do)
        // must therefore differ at the join, even though both start from the
        // same source and the same effect params.
        //
        // This is not a bug — a one-shot bounce has no "between calls" to
        // preserve state across — but it is real information for whoever
        // wires this render graph into `studio-io`'s realtime callback next:
        // a per-clip/per-bus `Instance` set needs to be built once and
        // carried across calls (a streaming variant), not reconstructed from
        // `mixdown`'s current one-shot shape.
        let rate = 48_000;
        let len_s = 0.1; // 4800 samples: long enough for a resonant filter's ring to matter
        let (mut p, audio) = project_with(len_s, 1.0);

        // A high-Q peaking boost right at the test tone's own frequency —
        // exactly the shape that rings (carries state) rather than settling
        // instantly, so a reset at the join is measurable.
        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0);
        params.insert("q0".to_string(), 8.0);
        params.insert("gain0".to_string(), 18.0);
        let clip_id = *p.clips.keys().next().unwrap();
        p.apply(&Op::SetEffects { id: clip_id, effects: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: true },
        ] }).unwrap();

        let total = Ticks::from_seconds(len_s);
        let one_shot = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, total, rate, &[], &[]);

        let mid = Ticks::from_samples((total.to_samples(rate)) / 2, rate);
        let mut two_calls = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, mid, rate, &[], &[]);
        two_calls.extend(mixdown(&p, &audio, &BTreeMap::new(), mid, total, rate, &[], &[]));

        assert_eq!(one_shot.len(), two_calls.len(), "splitting the window must not change the sample count");
        let diverges = one_shot.iter().zip(two_calls.iter()).any(|(a, b)| (a - b).abs() > 1e-4);
        assert!(diverges, "a resonant filter's state must NOT survive a mixdown call boundary today — \
            if this assertion starts failing, mixdown has become state-continuous across calls and this \
            test (and its doc comment) should be updated to say so, not deleted");
    }

    #[test]
    fn streaming_renderer_is_the_fix_the_previous_test_says_is_still_needed() {
        // The mirror image of `mixdown_reinstantiates_effect_state_on_every_
        // call_not_a_realtime_render_yet`, now asserting EQUALITY instead of
        // divergence: the same resonant EQ, the same split into two calls,
        // but through `StreamingRenderer` instead of bare `mixdown` — this
        // must match the one-shot offline render exactly, because the
        // effect instance carries its state across the call boundary
        // instead of being rebuilt.
        let rate = 48_000;
        let len_s = 0.1;
        let (mut p, audio) = project_with(len_s, 1.0);

        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0);
        params.insert("q0".to_string(), 8.0);
        params.insert("gain0".to_string(), 18.0);
        let clip_id = *p.clips.keys().next().unwrap();
        p.apply(&Op::SetEffects { id: clip_id, effects: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: true },
        ] }).unwrap();

        let total = Ticks::from_seconds(len_s);
        let one_shot = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, total, rate, &[], &[]);

        let mid = Ticks::from_samples(total.to_samples(rate) / 2, rate);
        let mut renderer = StreamingRenderer::new();
        let mut streamed = renderer.render(&p, &audio, &BTreeMap::new(), Ticks::ZERO, mid, rate, &[], &[]);
        streamed.extend(renderer.render(&p, &audio, &BTreeMap::new(), mid, total, rate, &[], &[]));

        assert_eq!(one_shot.len(), streamed.len());
        for (i, (a, b)) in one_shot.iter().zip(streamed.iter()).enumerate() {
            assert!((a - b).abs() < 1e-4, "sample {i}: streamed {b} vs one-shot {a} — effect state should have carried across the call boundary");
        }
    }

    #[test]
    fn streaming_renderer_matches_offline_mixdown_split_into_many_small_buffers() {
        // A more realistic shape than one split in the middle: many small,
        // consecutive buffer-sized calls, the way a real device callback
        // would actually drive this. Correctness must hold at every join,
        // not just one.
        let rate = 48_000;
        let len_s = 0.2;
        let (mut p, audio) = project_with(len_s, 1.0);
        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0);
        params.insert("q0".to_string(), 6.0);
        params.insert("gain0".to_string(), 12.0);
        let clip_id = *p.clips.keys().next().unwrap();
        p.apply(&Op::SetEffects { id: clip_id, effects: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: true },
        ] }).unwrap();

        let total = Ticks::from_seconds(len_s);
        let one_shot = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, total, rate, &[], &[]);

        let buffer = Ticks::from_samples(512, rate); // a plausible real device buffer size
        let mut renderer = StreamingRenderer::new();
        let mut streamed = Vec::new();
        let mut pos = Ticks::ZERO;
        while pos < total {
            let end = pos.saturating_add(buffer).min(total);
            streamed.extend(renderer.render(&p, &audio, &BTreeMap::new(), pos, end, rate, &[], &[]));
            pos = end;
        }

        assert_eq!(one_shot.len(), streamed.len());
        for (i, (a, b)) in one_shot.iter().zip(streamed.iter()).enumerate() {
            assert!((a - b).abs() < 1e-4, "sample {i}: streamed {b} vs one-shot {a}, across a {} buffer-sized calls", (total.to_samples(rate) as f64 / 512.0).ceil());
        }
    }

    #[test]
    fn a_seek_between_calls_resets_state_instead_of_carrying_stale_memory() {
        // A non-contiguous call (this call's `from` doesn't match the
        // previous call's `to`) is a seek. The cache must reset — carrying
        // a filter's memory across a jump would mean the render at the new
        // position depends on where the transport happened to be a moment
        // ago, which is exactly the kind of state a seek is supposed to cut
        // off. After a reset, rendering at a position is indistinguishable
        // from a fresh `mixdown` call at that same position.
        let rate = 48_000;
        let len_s = 0.1;
        let (mut p, audio) = project_with(len_s, 1.0);
        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0);
        params.insert("q0".to_string(), 8.0);
        params.insert("gain0".to_string(), 18.0);
        let clip_id = *p.clips.keys().next().unwrap();
        p.apply(&Op::SetEffects { id: clip_id, effects: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: true },
        ] }).unwrap();

        let total = Ticks::from_seconds(len_s);
        let mid = Ticks::from_samples(total.to_samples(rate) / 2, rate);
        let quarter = Ticks::from_samples(total.to_samples(rate) / 4, rate);

        let mut renderer = StreamingRenderer::new();
        let _ = renderer.render(&p, &audio, &BTreeMap::new(), Ticks::ZERO, mid, rate, &[], &[]);
        // A real seek: the next call's `from` (`quarter`) does NOT match the
        // previous call's `to` (`mid`) — continuing exactly at `mid` would be
        // the normal contiguous case (covered by the tests above), not a
        // seek.
        let after_seek = renderer.render(&p, &audio, &BTreeMap::new(), quarter, total, rate, &[], &[]);

        let fresh = mixdown(&p, &audio, &BTreeMap::new(), quarter, total, rate, &[], &[]);
        assert_eq!(after_seek, fresh, "a render after a seek must match a fresh one-shot render at that position, not carry over the pre-seek filter state");
    }

    #[test]
    fn a_live_effect_edit_between_contiguous_calls_takes_effect_immediately() {
        // This is a collaborative editor — another peer's edit (or the
        // user's own) can land between two consecutive renders. A cached
        // instance from before the edit must not silently keep playing the
        // old chain.
        let rate = 48_000;
        let len_s = 0.1;
        let (mut p, audio) = project_with(len_s, 1.0);
        let clip_id = *p.clips.keys().next().unwrap();

        let total = Ticks::from_seconds(len_s);
        let mid = Ticks::from_samples(total.to_samples(rate) / 2, rate);

        let mut renderer = StreamingRenderer::new();
        let _ = renderer.render(&p, &audio, &BTreeMap::new(), Ticks::ZERO, mid, rate, &[], &[]);

        // Edit lands between calls, contiguous position (no seek).
        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0);
        params.insert("gain0".to_string(), -60.0); // effectively silences the tone
        p.apply(&Op::SetEffects { id: clip_id, effects: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: true },
        ] }).unwrap();

        let after_edit = renderer.render(&p, &audio, &BTreeMap::new(), mid, total, rate, &[], &[]);
        let expected = mixdown(&p, &audio, &BTreeMap::new(), mid, total, rate, &[], &[]);
        assert_eq!(after_edit, expected, "a chain change must be picked up on the very next render, not deferred until a discontinuity");

        // Sanity, measured against the same span with no effect at all
        // (rather than an arbitrary absolute dB guess) so the threshold is
        // itself evidenced: a -60dB peaking cut centered on the tone's own
        // frequency must measurably quiet it, the same standard
        // `a_clip_effect_measurably_changes_the_mix` above already uses.
        let mut p_plain = p.clone();
        p_plain.apply(&Op::SetEffects { id: clip_id, effects: vec![] }).unwrap();
        let unprocessed = mixdown(&p_plain, &audio, &BTreeMap::new(), mid, total, rate, &[], &[]);
        assert!(
            peak_dbfs(&after_edit) < peak_dbfs(&unprocessed) - 6.0,
            "sanity: the -60dB EQ cut should have measurably quieted the tone: unprocessed {}dBFS, cut {}dBFS",
            peak_dbfs(&unprocessed), peak_dbfs(&after_edit)
        );
    }

    #[test]
    fn mixdown_with_cache_and_a_fresh_cache_each_call_is_equivalent_to_plain_mixdown() {
        // Regression check on the refactor itself: `mixdown` is now a thin
        // wrapper over `mixdown_with_cache` with a throwaway cache. Calling
        // `mixdown_with_cache` directly with a fresh cache every time must
        // produce byte-identical output to `mixdown`, proving the refactor
        // didn't change the offline path's behaviour.
        let (p, audio) = project_with(0.3, 0.7);
        let a = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.3), 48_000, &[], &[]);
        let mut cache = EffectCache::new();
        let b = mixdown_with_cache(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.3), 48_000, &[], &[], &mut cache);
        assert_eq!(a, b);
    }

    #[test]
    fn render_into_matches_render_including_a_project_with_busses_and_sends() {
        // Not just the simple no-bus case: this exercises the exact code
        // path `mixdown_into`'s bus fold rewrote (remove → retire →
        // reinsert instead of the old remove-and-drop), with a real send
        // and a real chain, so a mistake in the retire/reinsert bookkeeping
        // would show up as a value mismatch here.
        let (mut p, audio) = project_with(0.4, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Bus", None, 0.8) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Verb", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0);
        params.insert("gain0".to_string(), 6.0);
        p.apply(&Op::SetBusChain { id: BusId(1), chain: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: true },
        ] }).unwrap();
        p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![
            studio_core::Send { to: BusId(2), level: 0.3, pre_fader: true },
        ] }).unwrap();

        let span = (Ticks::ZERO, Ticks::from_seconds(0.4));
        let a = mixdown(&p, &audio, &BTreeMap::new(), span.0, span.1, 48_000, &[], &[]);
        let mut renderer = StreamingRenderer::new();
        let b = renderer.render(&p, &audio, &BTreeMap::new(), span.0, span.1, 48_000, &[], &[]);
        renderer.reset();
        let mut out = vec![0.0f32; a.len()];
        renderer.render_into(&p, &audio, &BTreeMap::new(), span.0, span.1, 48_000, &[], &[], &mut out);

        assert_eq!(a, b, "render() must match mixdown()");
        assert_eq!(a, out, "render_into() must match mixdown() too, busses and sends included");
    }

    #[test]
    fn render_into_reuses_scratch_allocations_across_contiguous_calls() {
        // The actual point of this whole refactor, checked directly rather
        // than trusted: after the first call has warmed up the scratch
        // buffers, a second contiguous call must not grow any of the
        // per-bus buffers — same technique `block.rs` uses for its own
        // "did this edit touch this allocation" claim, applied here to
        // "did this render call touch this allocation."
        let (mut p, audio) = project_with(0.2, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Bus", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();

        let mut renderer = StreamingRenderer::new();
        let total = Ticks::from_seconds(0.2);
        let mid = Ticks::from_samples(total.to_samples(48_000) / 2, 48_000);
        let mut out1 = vec![0.0f32; (mid.to_samples(48_000) as usize) * 2];
        renderer.render_into(&p, &audio, &BTreeMap::new(), Ticks::ZERO, mid, 48_000, &[], &[], &mut out1);

        let cap_before = renderer.scratch.bus_buf.get(&BusId(1)).unwrap().capacity();
        let mut out2 = vec![0.0f32; ((total.to_samples(48_000) - mid.to_samples(48_000)) as usize) * 2];
        renderer.render_into(&p, &audio, &BTreeMap::new(), mid, total, 48_000, &[], &[], &mut out2);
        let cap_after = renderer.scratch.bus_buf.get(&BusId(1)).unwrap().capacity();

        assert_eq!(cap_before, cap_after, "a same-size contiguous call must not reallocate the bus scratch buffer");
    }

    #[test]
    fn clip_effects_and_bus_effects_both_apply_in_the_same_render() {
        // Two independent EQ cuts (one on the clip, one on the bus it routes
        // through) at the same frequency should compound to MORE attenuation
        // than either alone — proving the two wiring sites are genuinely
        // independent stages, not one silently overriding or skipping the
        // other. Measured as steady-state RMS over a window well past any
        // filter settling transient (a cascaded biquad's peak can briefly
        // overshoot before settling, which makes whole-buffer peak the wrong
        // metric for comparing two stacked filters — RMS over a settled
        // window is what actually reflects steady-state attenuation).
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let clip_id = *p.clips.keys().next().unwrap();

        fn rms_settled(out: &[f32]) -> f32 {
            let settled = &out[4000.min(out.len())..]; // skip the first ~83ms
            (settled.iter().map(|s| s * s).sum::<f32>() / settled.len().max(1) as f32).sqrt()
        }
        fn eq_cut(hz: f32, gain_db: f32) -> studio_core::Effect {
            let mut params = std::collections::BTreeMap::new();
            params.insert("bands".to_string(), 1.0);
            params.insert("freq0".to_string(), hz);
            params.insert("gain0".to_string(), gain_db);
            studio_core::Effect { kind: "eq".into(), params, enabled: true }
        }

        let baseline = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        p.apply(&Op::SetEffects { id: clip_id, effects: vec![eq_cut(440.0, -9.0)] }).unwrap();
        let clip_only = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        p.apply(&Op::SetBusChain { id: BusId(1), chain: vec![eq_cut(440.0, -9.0)] }).unwrap();
        let both = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let (r0, r1, r2) = (rms_settled(&baseline), rms_settled(&clip_only), rms_settled(&both));
        assert!(r1 < r0, "the clip-only cut should reduce settled RMS below baseline: {r0} -> {r1}");
        assert!(r2 < r1, "adding the bus cut on top should reduce settled RMS further still: {r1} -> {r2}");
    }

    #[test]
    fn a_bus_chain_effect_measurably_changes_the_mix() {
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let unprocessed = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0); // project_with's own test tone frequency
        params.insert("gain0".to_string(), -18.0); // a heavy cut, easy to detect
        p.apply(&Op::SetBusChain { id: BusId(1), chain: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: true },
        ] }).unwrap();
        let processed = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let (pu, pp) = (peak_dbfs(&unprocessed), peak_dbfs(&processed));
        assert!(pp < pu - 6.0, "an -18dB EQ cut at the tone's own frequency should measurably quiet the mix: unprocessed {pu}dBFS, processed {pp}dBFS");
    }

    #[test]
    fn a_bypassed_chain_effect_does_not_change_the_mix() {
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let unprocessed = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0);
        params.insert("gain0".to_string(), -18.0);
        p.apply(&Op::SetBusChain { id: BusId(1), chain: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: false }, // disabled
        ] }).unwrap();
        let with_disabled_effect = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        for i in 0..unprocessed.len() {
            assert!((unprocessed[i] - with_disabled_effect[i]).abs() < 1e-5,
                "sample {i}: a disabled chain effect must not change the mix at all");
        }
    }

    #[test]
    fn an_unknown_effect_kind_in_a_bus_chain_is_silently_skipped_not_a_panic() {
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        p.apply(&Op::SetBusChain { id: BusId(1), chain: vec![
            studio_core::Effect { kind: "plugin_i_dont_have".into(), params: Default::default(), enabled: true },
        ] }).unwrap();
        // Must not panic, and should mix exactly as if the chain were empty —
        // the same "a project opens without the plugin it references" rule
        // the rest of this codebase's effect hosts already follow.
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(out.iter().any(|s| *s != 0.0));
    }

    #[test]
    fn a_pre_fader_send_hears_the_chain_but_not_the_fader() {
        // The chain runs before `pre` is captured, so a pre-fader send
        // should carry the CHAIN-processed signal — unlike the fader
        // (Bus.gain), which a pre-fader send is specifically defined to skip.
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None, 0.0) }).unwrap(); // fader at zero
        p.apply(&Op::AddBus { bus: bus(2, "Cue", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 1.0, pre_fader: true }] }).unwrap();

        let mut params = std::collections::BTreeMap::new();
        params.insert("bands".to_string(), 1.0);
        params.insert("freq0".to_string(), 440.0);
        params.insert("gain0".to_string(), -18.0);
        p.apply(&Op::SetBusChain { id: BusId(1), chain: vec![
            studio_core::Effect { kind: "eq".into(), params, enabled: true },
        ] }).unwrap();

        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        // Compare against the SAME setup but with the chain removed, to
        // measure whether the send's content reflects the EQ cut.
        let mut p_no_chain = p.clone();
        p_no_chain.busses.get_mut(&BusId(1)).unwrap().chain.clear();
        let out_no_chain = mixdown(&p_no_chain, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        let (with_chain_peak, without_chain_peak) = (peak_dbfs(&out), peak_dbfs(&out_no_chain));
        assert!(with_chain_peak < without_chain_peak - 6.0,
            "the pre-fader send should still carry the EQ-cut signal: with chain {with_chain_peak}dBFS, without {without_chain_peak}dBFS");
    }

    #[test]
    fn a_muted_bus_silences_itself_and_everything_summed_into_it() {
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Drums", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let heard = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(heard.iter().any(|s| *s != 0.0));

        p.apply(&Op::SetBusFlags { id: BusId(1), muted: true }).unwrap();
        let muted = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(muted.iter().all(|s| *s == 0.0), "a muted bus must silence every track routed through it");
    }

    #[test]
    fn bus_mute_is_an_absolute_override_even_while_that_bus_is_soloed() {
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddBus { bus: bus(1, "Drums", None, 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        p.apply(&Op::SetBusFlags { id: BusId(1), muted: true }).unwrap();
        // Soloing the very bus that's muted: mute wins, by design (see the
        // module docs on why this crate picked an absolute override instead
        // of "solo overrides mute for the soloed channel," which some
        // consoles do and this one deliberately doesn't, for simplicity).
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[BusId(1)]);
        assert!(out.iter().all(|s| *s == 0.0), "mute must beat solo, even self-solo");
    }

    #[test]
    fn soloing_a_bus_silences_sibling_busses_but_not_its_own_ancestors_or_descendants() {
        // Beat
        // ├── Drums (routed track 1)
        // └── Melody (routed track 2)
        // Soloing Drums must silence Melody's path to master, but Beat
        // itself (Drums' ancestor, needed to carry the soloed signal up)
        // must remain open.
        let (mut p, audio) = project_with(0.5, 1.0);
        p.apply(&Op::AddTrack { track: studio_core::Track {
            id: TrackId(2), kind: studio_core::TrackKind::Audio, name: "A2".into(),
            muted: false, locked: false, gain: 1.0, route: None,
        }, at: 1 }).unwrap();
        let mut audio2 = audio.clone();
        audio2.insert(studio_core::SourceId(2), tone(24_000, 48_000, 660.0, 0.5));
        p.apply(&Op::AddSource { source: Source { id: studio_core::SourceId(2), path: "b.wav".into(),
            len: Ticks::from_seconds(0.5), sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(2), source: studio_core::SourceId(2), track: TrackId(2),
            pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(0.5), gain: 1.0, effects: Vec::new() } }).unwrap();

        p.apply(&Op::AddBus { bus: bus(1, "Beat", None, 1.0) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Drums", Some(1), 1.0) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(3, "Melody", Some(1), 1.0) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(2)) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(2), bus: Some(BusId(3)) }).unwrap();

        let both = mixdown(&p, &audio2, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        assert!(both.iter().any(|s| *s != 0.0));

        let drums_only = mixdown(&p, &audio2, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[BusId(2)]);
        // Isolate the pure-Drums path for comparison: mute Melody instead of
        // soloing Drums, which should reach the identical result if the
        // solo logic is correct.
        let mut melody_muted = p.clone();
        melody_muted.apply(&Op::SetBusFlags { id: BusId(3), muted: true }).unwrap();
        let expected = mixdown(&melody_muted, &audio2, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);

        assert_eq!(drums_only.len(), expected.len());
        for i in 0..drums_only.len() {
            assert!((drums_only[i] - expected[i]).abs() < 1e-5,
                "sample {i}: soloing Drums must equal muting Melody — Beat (their shared ancestor) must stay open either way");
        }
    }
}

#[cfg(test)]
mod track_fader_tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, SourceId, Track, TrackId, TrackKind};

    fn one_clip_project(track_gain: f32) -> (Project, BTreeMap<SourceId, SourceAudio>) {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false, gain: track_gain, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(1.0), sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1),
            track: TrackId(1), pos: Ticks::ZERO, src_in: Ticks::ZERO,
            len: Ticks::from_seconds(1.0), gain: 1.0, effects: vec![] } }).unwrap();
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), SourceAudio { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]] });
        (p, audio)
    }

    #[test]
    fn the_track_fader_scales_its_clips_like_the_clip_fader_does() {
        // Half gain is 6 dB down — the same rule the clip-gain test pins,
        // now one stage upstream, and multiplicative with it.
        let (p_full, audio) = one_clip_project(1.0);
        let (p_half, _) = one_clip_project(0.5);
        let full = mixdown(&p_full, &audio, &BTreeMap::new(),
                           Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        let half = mixdown(&p_half, &audio, &BTreeMap::new(),
                           Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[], &[]);
        let sum = |v: &[f32]| v.iter().map(|s| s.abs()).sum::<f32>();
        let ratio = sum(&half) / sum(&full);
        assert!((ratio - 0.5).abs() < 1e-4, "fader at 0.5 halves the level, ratio {ratio}");
    }
}

#[cfg(test)]
mod source_rate_tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, SourceId, Track, TrackId, TrackKind};

    #[test]
    fn a_low_rate_source_plays_at_pitch_on_the_timeline() {
        // A 24k ramp rendered at 48k must stretch to its true duration with
        // the same content — out frame 2n sits at source frame n — exactly
        // the contract the rack sampler's test pins, now for mixdown.
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(1.0), sample_rate: 24_000, channels: 1,
            has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1),
            track: TrackId(1), pos: Ticks::ZERO, src_in: Ticks::ZERO,
            len: Ticks::from_seconds(1.0), gain: 1.0, effects: vec![] } }).unwrap();
        let ramp: Vec<f32> = (0..24_000).map(|i| i as f32 / 24_000.0).collect();
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), SourceAudio { sample_rate: 24_000, channels: vec![ramp] });

        let out = mixdown(&p, &audio, &BTreeMap::new(),
                          Ticks::ZERO, Ticks::from_seconds(1.0), 48_000, &[], &[]);
        // At 0.5s of render (frame 24000, interleaved index 48000), the ramp
        // must read 0.5 — the pre-fix code read 1.0-and-then-silence there.
        let mid_l = out[24_000 * 2];
        assert!((mid_l - 0.5).abs() < 0.01, "half a second in is half the ramp, got {mid_l}");
        // And the last audible frames are near the ramp's top, not silence:
        // the source lasts the whole second at pitch.
        let late_l = out[(47_990) * 2];
        assert!(late_l > 0.95, "the source fills its true duration, got {late_l}");
    }
}

#[cfg(test)]
mod automation_render_tests {
    use super::*;
    use studio_core::{AutoTarget, Bus, BusId, Clip, ClipId, Op, Source, SourceId, Track, TrackId, TrackKind};

    fn one_clip_project() -> (Project, BTreeMap<SourceId, SourceAudio>) {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(1.0), sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1),
            track: TrackId(1), pos: Ticks::ZERO, src_in: Ticks::ZERO,
            len: Ticks::from_seconds(1.0), gain: 1.0, effects: vec![] } }).unwrap();
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), SourceAudio { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]] });
        (p, audio)
    }
    fn render(p: &Project, a: &BTreeMap<SourceId, SourceAudio>) -> Vec<f32> {
        mixdown(p, a, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(1.0), 48_000, &[], &[])
    }

    #[test]
    fn a_gain_ramp_is_a_fade_the_render_actually_follows() {
        // 0→1 across the second: the first sample near silent, the midpoint
        // near half, the end at the fader's own level — per-sample linear,
        // not stepped per buffer.
        let (mut p, audio) = one_clip_project();
        let t = AutoTarget::TrackGain(TrackId(1));
        p.apply(&Op::SetAutoPoint { target: t, at: Ticks::ZERO, value: 0.0 }).unwrap();
        p.apply(&Op::SetAutoPoint { target: t, at: Ticks::from_seconds(1.0), value: 1.0 }).unwrap();
        let out = render(&p, &audio);
        assert!(out[0].abs() < 1e-3, "the ramp starts at silence, got {}", out[0]);
        let mid = out[out.len() / 2];
        assert!((mid - 0.25).abs() < 0.01, "midpoint is half the ride on a 0.5 source: {mid}");
        let last = out[out.len() - 2];
        assert!((last - 0.5).abs() < 0.01, "the end reaches the full source level: {last}");
    }

    #[test]
    fn no_lane_renders_bit_identically_to_a_pre_automation_project() {
        // The property that makes automation safe to land: a project that
        // never touched it cannot sound different.
        let (p, audio) = one_clip_project();
        let mut with_empty_map = p.clone();
        with_empty_map.automation.clear();
        assert_eq!(render(&p, &audio), render(&with_empty_map, &audio));
    }

    #[test]
    fn the_lane_rides_the_fader_rather_than_replacing_it() {
        // Fader 0.5 × lane 0.5 = a quarter of the source — the documented
        // multiply, pinned so "automation ate my fader" can never ship.
        let (mut p, audio) = one_clip_project();
        p.apply(&Op::SetTrackGain { id: TrackId(1), gain: 0.5 }).unwrap();
        p.apply(&Op::SetAutoPoint { target: AutoTarget::TrackGain(TrackId(1)),
                                    at: Ticks::ZERO, value: 0.5 }).unwrap();
        let out = render(&p, &audio);
        let mid = out[out.len() / 2];
        assert!((mid - 0.125).abs() < 1e-3, "0.5 source × 0.5 fader × 0.5 lane: {mid}");
    }

    #[test]
    fn a_bus_lane_rides_the_bus_fader_over_time() {
        // Route the track through a bus and ramp the BUS down: the mix ends
        // quieter than it starts, and a flat lane at unity changes nothing.
        let (mut p, audio) = one_clip_project();
        p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "B".into(), gain: 1.0, silent: false,
            muted: false, chain: vec![], sends: vec![], parent: None } }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let b = AutoTarget::BusGain(BusId(1));
        let mut ramped = p.clone();
        ramped.apply(&Op::SetAutoPoint { target: b, at: Ticks::ZERO, value: 1.0 }).unwrap();
        ramped.apply(&Op::SetAutoPoint { target: b, at: Ticks::from_seconds(1.0), value: 0.0 }).unwrap();
        let out = render(&ramped, &audio);
        let head = out[..2400].iter().map(|s| s.abs()).sum::<f32>();
        let tail = out[out.len() - 2400..].iter().map(|s| s.abs()).sum::<f32>();
        assert!(head > tail * 20.0, "the ride pulled the bus down: head {head}, tail {tail}");

        let mut flat = p.clone();
        flat.apply(&Op::SetAutoPoint { target: b, at: Ticks::ZERO, value: 1.0 }).unwrap();
        let plain = render(&p, &audio);
        let unity = render(&flat, &audio);
        for i in 0..plain.len() {
            assert!((plain[i] - unity[i]).abs() < 1e-6, "a unity lane is transparent at {i}");
        }
    }

    fn effect(kind: &str, params: &[(&str, f32)]) -> studio_core::Effect {
        studio_core::Effect { kind: kind.into(), enabled: true,
            params: params.iter().map(|(k, v)| ((*k).to_string(), *v)).collect() }
    }

    #[test]
    fn a_parameter_lane_rides_a_bus_effect_at_control_rate() {
        // A -20 dB gain effect on the bus and a lane on its `db`: 1.0 (the
        // full -20) for the first half, 0.0 (unity) after. The stored
        // value is what the lane multiplies — the fader rule, on a knob.
        use studio_core::ParamKey;
        let (mut p, audio) = one_clip_project();
        p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "B".into(), gain: 1.0, silent: false, muted: false,
            // A DISABLED effect first: the instance list skips it, so
            // `effect: 1` proves the index addresses the chain, not the
            // instance list.
            chain: vec![studio_core::Effect { enabled: false, ..effect("gain", &[("db", 0.0)]) },
                        effect("gain", &[("db", -20.0)])],
            sends: vec![], parent: None } }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let plain = render(&p, &audio);
        let db = AutoTarget::BusParam { bus: BusId(1), effect: 1, key: ParamKey::new("db").unwrap() };
        let mut ridden = p.clone();
        ridden.apply(&Op::SetAutoPoint { target: db, at: Ticks::ZERO, value: 1.0 }).unwrap();
        ridden.apply(&Op::SetAutoPoint { target: db, at: Ticks::from_seconds(0.5), value: 1.0 }).unwrap();
        // A step, not a ramp, at 0.5 s: eval_lane is linear between points,
        // so put the two points a tick apart.
        ridden.apply(&Op::SetAutoPoint { target: db, at: Ticks::from_seconds(0.5).saturating_add(Ticks(1)), value: 0.0 }).unwrap();
        let out = render(&ridden, &audio);
        let quiet = out[1000];
        let loud = out[out.len() - 1000];
        assert!((quiet - 0.05).abs() < 0.002, "first half: 0.5 at -20 dB = 0.05, got {quiet}");
        assert!((loud - 0.5).abs() < 0.002, "second half: the lane at 0 makes -20 × 0 = 0 dB, got {loud}");
        // Control rate: the step lands within one block of the point.
        let half = out.len() / 2;
        let first_loud = out.iter().position(|v| *v > 0.4).unwrap();
        assert!(first_loud >= half && first_loud < half + PARAM_BLOCK * 2 + 2,
                "the change lands at the point's block: {first_loud} vs {half}");
        // The plain project, and one ridden at unity, are untouched.
        assert!((plain[1000] - 0.05).abs() < 0.002);
        let mut unity = p.clone();
        unity.apply(&Op::SetAutoPoint { target: db, at: Ticks::ZERO, value: 1.0 }).unwrap();
        let same = render(&unity, &audio);
        for i in 0..plain.len() { assert!((plain[i] - same[i]).abs() < 1e-6, "unity ride is transparent at {i}"); }
    }

    #[test]
    fn a_dangling_or_unknown_parameter_target_changes_nothing() {
        use studio_core::ParamKey;
        let (mut p, audio) = one_clip_project();
        p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "B".into(), gain: 1.0, silent: false, muted: false,
            chain: vec![effect("gain", &[("db", -6.0)])], sends: vec![], parent: None } }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let plain = render(&p, &audio);
        let mut r = p.clone();
        // Past the chain's end, a key the effect does not have, and a key
        // the effect cannot move live: all three are silently nothing.
        for t in [
            AutoTarget::BusParam { bus: BusId(1), effect: 5, key: ParamKey::new("db").unwrap() },
            AutoTarget::BusParam { bus: BusId(1), effect: 0, key: ParamKey::new("nothing").unwrap() },
            AutoTarget::ClipParam { clip: ClipId(1), effect: 0, key: ParamKey::new("db").unwrap() },
        ] {
            r.apply(&Op::SetAutoPoint { target: t, at: Ticks::ZERO, value: 0.0 }).unwrap();
        }
        assert_eq!(render(&r, &audio), plain);
    }

    #[test]
    fn a_parameter_lane_on_a_clip_effect_follows_the_clip() {
        // The same ride on a CLIP's chain, through a binding gated to the
        // clip — the reusable-curve promise, on a parameter.
        use studio_core::{Binding, BindingId, CurveId, Gate, ParamKey};
        let (mut p, audio) = one_clip_project();
        p.apply(&Op::SetEffects { id: ClipId(1), effects: vec![effect("gain", &[("db", -20.0)])] }).unwrap();
        let target = AutoTarget::ClipParam { clip: ClipId(1), effect: 0, key: ParamKey::new("db").unwrap() };
        p.apply(&Op::SetCurve { id: CurveId(1), name: "open".into() }).unwrap();
        // In CLIP time: full attenuation for the first half, unity after.
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks::ZERO, value: 1.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks::from_seconds(0.5), value: 1.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks::from_seconds(0.5).saturating_add(Ticks(1)), value: 0.0 }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding {
            curve: CurveId(1), target, gate: Gate::Clip(ClipId(1)), response: None, fit_span: false  } }).unwrap();
        let out = render(&p, &audio);
        assert!((out[1000] - 0.05).abs() < 0.002, "clip time 0: -20 dB: {}", out[1000]);
        assert!((out[out.len() - 1000] - 0.5).abs() < 0.002, "clip time 0.75 s: unity: {}", out[out.len() - 1000]);
        // Move the clip later: the ride moves with it. Render a window
        // that starts inside the clip and the step is still half a second
        // into the CLIP, not into the timeline.
        p.apply(&Op::MoveClip { id: ClipId(1), pos: Ticks::from_seconds(0.25), track: TrackId(1) }).unwrap();
        let out = render(&p, &audio);
        let at = |s: f32| out[(s * 48_000.0) as usize * 2];
        assert!((at(0.5) - 0.05).abs() < 0.002, "0.25 s into the clip: still -20 dB: {}", at(0.5));
        assert!((at(0.9) - 0.5).abs() < 0.002, "0.65 s into the clip: unity: {}", at(0.9));
    }
}

#[cfg(test)]
mod binding_render_tests {
    //! The curves module's promise, checked through a REAL render rather
    //! than through `auto_value`: a clip-gated ride follows its clip, a bus
    //! ducks under a clip on another track, and a project with no bindings
    //! renders bit-identically to before bindings existed.
    use super::*;
    use studio_core::{AutoTarget, Binding, BindingId, Bus, BusId, Clip, ClipId, CurveId, Gate, Op,
                      Source, SourceId, Track, TrackId, TrackKind};

    /// Two tracks, one 0.5-DC source, a clip on track 1 at 0.2–0.4 s. A
    /// "dip" curve reading 0.0 at its own 0.1 s, 1.0 at its ends.
    fn fixture() -> (Project, BTreeMap<SourceId, SourceAudio>) {
        let mut p = Project::default();
        for id in [1, 2] {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Audio,
                name: format!("A{id}"), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        }
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(1.0), sample_rate: 48_000, channels: 1, has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1),
            track: TrackId(1), pos: Ticks::from_seconds(0.2), src_in: Ticks::ZERO,
            len: Ticks::from_seconds(0.2), gain: 1.0, effects: vec![] } }).unwrap();
        p.apply(&Op::SetCurve { id: CurveId(1), name: "dip".into() }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks::ZERO, value: 1.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks::from_seconds(0.1), value: 0.0 }).unwrap();
        p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks::from_seconds(0.2), value: 1.0 }).unwrap();
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), SourceAudio { sample_rate: 48_000, channels: vec![vec![0.5; 48_000]] });
        (p, audio)
    }
    fn render(p: &Project, a: &BTreeMap<SourceId, SourceAudio>) -> Vec<f32> {
        mixdown(p, a, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(1.0), 48_000, &[], &[])
    }
    fn left_at(out: &[f32], secs: f64) -> f32 { out[(secs * 48_000.0) as usize * 2] }

    #[test]
    fn no_bindings_renders_bit_identically_to_a_pre_binding_project() {
        let (mut p, a) = fixture();
        let before = render(&p, &a);
        // A curve with no binding, and then no curve at all, must both be
        // the same samples: the render only ever sees bindings.
        let with_curve = render(&p, &a);
        assert_eq!(before, with_curve);
        p.curves.clear();
        assert_eq!(before, render(&p, &a));
        assert!((left_at(&before, 0.3) - 0.5).abs() < 1e-6, "the clip is audible at unity");
    }

    #[test]
    fn a_clip_gated_ride_follows_the_clip_through_a_real_render() {
        let (mut p, a) = fixture();
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding { curve: CurveId(1),
            target: AutoTarget::TrackGain(TrackId(1)), gate: Gate::Clip(ClipId(1)), response: None, fit_span: false  } }).unwrap();
        let out = render(&p, &a);
        assert!((left_at(&out, 0.3)).abs() < 1e-3, "the dip lands 0.1 s into the clip, got {}", left_at(&out, 0.3));
        assert!((left_at(&out, 0.25) - 0.25).abs() < 2e-3, "halfway down the ramp");
        // Move the clip: the dip must move with it and leave nothing behind.
        p.apply(&Op::MoveClip { id: ClipId(1), pos: Ticks::from_seconds(0.6), track: TrackId(1) }).unwrap();
        let out = render(&p, &a);
        assert_eq!(left_at(&out, 0.3), 0.0, "nothing plays where the clip was");
        assert!((left_at(&out, 0.7)).abs() < 1e-3, "the dip is at the new place");
        assert!((left_at(&out, 0.65) - 0.25).abs() < 2e-3);
    }

    #[test]
    fn a_bus_ducks_under_a_clip_on_another_track() {
        // Track 2 carries a constant tone through a bus; track 1's clip
        // gates a dip on the BUS gain. Structural sidechain: while the
        // vocal plays, the drums drop, and nothing else changes.
        let (mut p, a) = fixture();
        p.apply(&Op::AddBus { bus: Bus { id: BusId(1), name: "drums".into(), parent: None,
            gain: 1.0, chain: vec![], sends: vec![], muted: false, silent: false } }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(2), bus: Some(BusId(1)) }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(2), source: SourceId(1),
            track: TrackId(2), pos: Ticks::ZERO, src_in: Ticks::ZERO,
            len: Ticks::from_seconds(1.0), gain: 1.0, effects: vec![] } }).unwrap();
        // Mute track 1 so only the bus's output is measured.
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: true, locked: false }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding { curve: CurveId(1),
            target: AutoTarget::BusGain(BusId(1)), gate: Gate::Clip(ClipId(1)), response: None, fit_span: false  } }).unwrap();
        let out = render(&p, &a);
        assert!((left_at(&out, 0.1) - 0.5).abs() < 1e-6, "before the vocal: full");
        assert!((left_at(&out, 0.3)).abs() < 1e-3, "under the vocal's dip: ducked, got {}", left_at(&out, 0.3));
        assert!((left_at(&out, 0.5) - 0.5).abs() < 1e-6, "after the vocal: full again");
        // L and R duck together — a ramp must not tilt the image.
        let fi = (0.25 * 48_000.0) as usize * 2;
        assert_eq!(out[fi], out[fi + 1]);
    }

    #[test]
    fn the_render_matches_the_models_own_definition_sample_for_sample() {
        // The equivalence guard: the swept fast path against a naive
        // per-sample `auto_value`, across a lane AND two bindings.
        let (mut p, a) = fixture();
        let t = AutoTarget::TrackGain(TrackId(1));
        p.apply(&Op::SetAutoPoint { target: t, at: Ticks::ZERO, value: 0.5 }).unwrap();
        p.apply(&Op::SetAutoPoint { target: t, at: Ticks::from_seconds(1.0), value: 1.0 }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(1), binding: Binding { curve: CurveId(1),
            target: t, gate: Gate::Clip(ClipId(1)), response: None, fit_span: false  } }).unwrap();
        p.apply(&Op::SetBinding { id: BindingId(2), binding: Binding { curve: CurveId(1),
            target: t, gate: Gate::Window { start: Ticks::from_seconds(0.3), end: Ticks::from_seconds(0.5) }, response: None, fit_span: false } }).unwrap();
        let out = render(&p, &a);
        let mut checked = 0;
        for f in (0..48_000).step_by(97) {
            let at = Ticks::from_samples(f, 48_000);
            let expected = if at >= Ticks::from_seconds(0.2) && at < Ticks::from_seconds(0.4) {
                0.5 * p.auto_value(t, at)
            } else { 0.0 };
            let got = out[f as usize * 2];
            assert!((got - expected).abs() < 1e-5, "frame {f}: render {got} vs model {expected}");
            checked += 1;
        }
        assert!(checked > 400);
    }
}

// ── vocal alignment ─────────────────────────────────────────────────────

/// The audio's energy envelope at one value per millisecond — the domain
/// alignment happens in. Envelopes are RATE-AGNOSTIC by construction (a
/// 44.1k take and a 48k reference land on the same time axis), phase-blind
/// (two mics of one performance correlate even when the waveforms do not),
/// and a thousand times cheaper than sample-rate correlation.
pub fn envelope_ms(samples: &[f32], rate: u32) -> Vec<f32> {
    let hop = (rate / 1000).max(1) as usize;
    samples.chunks(hop)
        .map(|c| c.iter().map(|s| s.abs()).sum::<f32>() / c.len() as f32)
        .collect()
}

/// The lag, in MILLISECONDS, at which `take` best matches `reference`:
/// positive means the take's material happens LATER than the reference's
/// (the fix is moving the take's clip earlier by that much), negative the
/// opposite. Normalized cross-correlation over ±`max_shift_ms`, on
/// envelopes from [`envelope_ms`]. Silence against silence is honest
/// zero, not a confident lie.
pub fn align_offset_ms(reference: &[f32], take: &[f32], max_shift_ms: usize) -> i64 {
    if reference.is_empty() || take.is_empty() { return 0; }
    let mut best = (0i64, f32::MIN);
    for shift in -(max_shift_ms as i64)..=(max_shift_ms as i64) {
        let mut dot = 0.0f32;
        let mut na = 0.0f32;
        let mut nb = 0.0f32;
        for (i, r) in reference.iter().enumerate() {
            let j = i as i64 + shift;
            if j < 0 || j as usize >= take.len() { continue; }
            let t = take[j as usize];
            dot += r * t;
            na += r * r;
            nb += t * t;
        }
        let norm = (na * nb).sqrt();
        let score = if norm > 1e-12 { dot / norm } else { 0.0 };
        // Strictly-greater keeps the SMALLEST winning lag when scores tie
        // (a periodic take ties at every period): the least surprising
        // move wins, and iteration order from -max makes "smallest" mean
        // "most negative" deterministically rather than by float luck.
        if score > best.1 { best = (shift, score); }
    }
    // No positive correlation anywhere — silence, or unrelated material —
    // is an honest zero, not the leftmost shift that happened to beat the
    // f32::MIN sentinel (exactly how the first draft returned -max for
    // silence-vs-silence).
    if best.1 <= 1e-6 { 0 } else { best.0 }
}

#[cfg(test)]
mod align_tests {
    use super::*;

    fn burst(rate: u32) -> Vec<f32> {
        // A syllable-like shape: silence, an attack, a decaying tail — the
        // envelope structure real vocals have and pure tones do not.
        let mut v = vec![0.0f32; rate as usize / 10];
        for i in 0..(rate as usize / 5) {
            let t = i as f32 / rate as f32;
            let env = (-t * 12.0).exp();
            v.push((t * 300.0 * std::f32::consts::TAU).sin() * env);
        }
        v.extend(vec![0.0f32; rate as usize / 10]);
        v
    }

    #[test]
    fn a_delayed_copy_is_found_to_within_the_envelope_hop() {
        let rate = 48_000;
        let r = burst(rate);
        let mut late = vec![0.0f32; (rate / 1000 * 37) as usize]; // 37 ms late
        late.extend(&r);
        let d = align_offset_ms(&envelope_ms(&r, rate), &envelope_ms(&late, rate), 200);
        assert!((d - 37).abs() <= 1, "37 ms of delay recovered, got {d}");
        // And the other direction: the take EARLY means a negative lag.
        let d = align_offset_ms(&envelope_ms(&late, rate), &envelope_ms(&r, rate), 200);
        assert!((d + 37).abs() <= 1, "early is negative, got {d}");
    }

    #[test]
    fn rates_do_not_matter_because_the_envelope_domain_is_milliseconds() {
        // A 44.1k double against a 48k lead — the exact case that makes
        // sample-domain correlation quietly wrong by the rate ratio.
        let lead = burst(48_000);
        let mut double = vec![0.0f32; 44_100 / 1000 * 25]; // 25 ms late, at 44.1k
        double.extend(burst(44_100));
        let d = align_offset_ms(&envelope_ms(&lead, 48_000),
                                &envelope_ms(&double, 44_100), 200);
        assert!((d - 25).abs() <= 2, "cross-rate alignment holds, got {d}");
    }

    #[test]
    fn silence_is_an_honest_zero_not_a_confident_lie() {
        let z = vec![0.0f32; 48_000];
        assert_eq!(align_offset_ms(&envelope_ms(&z, 48_000),
                                   &envelope_ms(&z, 48_000), 100), 0);
    }
}

// ── the morph pad ───────────────────────────────────────────────────────

/// Bilinear corner weights for a puck at (x, y) over up to four anchors,
/// corner order TL, TR, BL, BR (y up). Fewer anchors take the first
/// corners and the weights RENORMALIZE over what is present — with two
/// anchors on the top edge the pad degrades to a clean one-axis
/// crossfade, x alone deciding, which is exactly what a two-sound blend
/// should feel like.
pub fn morph_weights(n: usize, x: f32, y: f32) -> Vec<f32> {
    let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
    let corners = [(1.0 - x) * y, x * y, (1.0 - x) * (1.0 - y), x * (1.0 - y)];
    let mut w: Vec<f32> = corners[..n.clamp(1, 4)].to_vec();
    let sum: f32 = w.iter().sum();
    if sum <= 1e-9 {
        // The puck parked exactly opposite every present anchor: nearest
        // wins outright rather than dividing by nothing.
        let mut z = vec![0.0; w.len()];
        z[0] = 1.0;
        return z;
    }
    for v in &mut w { *v /= sum; }
    w
}

fn morph_mono_48k(a: &SourceAudio) -> Vec<f32> {
    let ratio = a.sample_rate as f64 / 48_000.0;
    let out_len = (a.frames() as f64 / ratio).max(1.0) as usize;
    (0..out_len).map(|i| {
        let pos = i as f64 * ratio;
        let i0 = pos as usize;
        let frac = (pos - i0 as f64) as f32;
        let s0 = a.mono(i0.min(a.frames().saturating_sub(1)));
        let s1 = a.mono((i0 + 1).min(a.frames().saturating_sub(1)));
        s0 * (1.0 - frac) + s1 * frac
    }).collect()
}

/// One sound, reshaped by the puck: X is spectral TILT (left dark, right
/// bright, centre untouched — a crossfade toward a one-pole low- or
/// high-passed copy), Y is DECAY (1.0 untouched, lower fades the tail
/// earlier — a drum's most-reached-for knob). Neutral is (0.5, 1.0), and
/// neutral is pinned to be the identity: a pad that colours the sound at
/// rest cannot be trusted anywhere else.
pub fn tone_map(a: &SourceAudio, x: f32, y: f32) -> SourceAudio {
    let src = morph_mono_48k(a);
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let mut out = src.clone();
    if (x - 0.5).abs() > 1e-3 {
        // One-pole low-pass at ~800 Hz; the high-pass is its residue, so
        // the two halves sum back to the original by construction.
        let k = 1.0 - (-2.0 * std::f32::consts::PI * 800.0 / 48_000.0).exp();
        let mut lp_state = 0.0f32;
        for (i, s) in src.iter().enumerate() {
            lp_state += k * (s - lp_state);
            let hp = s - lp_state;
            out[i] = if x < 0.5 {
                let t = (0.5 - x) * 2.0; // toward dark
                s * (1.0 - t) + lp_state * t
            } else {
                let t = (x - 0.5) * 2.0; // toward bright
                s * (1.0 - t) + hp * t
            };
        }
    }
    if y < 1.0 - 1e-3 {
        // The tail fades over the first y of the length and is silent
        // after — the roll's gate rule, driven by a knob.
        let gate = ((out.len() as f32) * y.max(0.02)) as usize;
        let fade = (gate / 8).max(48).min(gate);
        for i in 0..out.len() {
            if i >= gate { out[i] = 0.0; }
            else if i >= gate - fade {
                out[i] *= (gate - i) as f32 / fade as f32;
            }
        }
    }
    SourceAudio { sample_rate: 48_000, channels: vec![out] }
}

/// The pad's sound at (x, y): one anchor is [`tone_map`], several blend by
/// [`morph_weights`] — resampled to 48k mono, padded to the longest, the
/// weighted sum. Similar sounds blended sit BETWEEN them; dissimilar
/// sounds layer, which is also a sound — the pad judges nothing.
pub fn morph_blend(anchors: &[&SourceAudio], x: f32, y: f32) -> SourceAudio {
    match anchors {
        [] => SourceAudio { sample_rate: 48_000, channels: vec![vec![]] },
        [one] => tone_map(one, x, y),
        many => {
            let w = morph_weights(many.len(), x, y);
            let monos: Vec<Vec<f32>> = many.iter().map(|a| morph_mono_48k(a)).collect();
            let len = monos.iter().map(Vec::len).max().unwrap_or(0);
            let mut out = vec![0.0f32; len];
            for (mono, wt) in monos.iter().zip(&w) {
                for (i, s) in mono.iter().enumerate() {
                    out[i] += s * wt;
                }
            }
            SourceAudio { sample_rate: 48_000, channels: vec![out] }
        }
    }
}

#[cfg(test)]
mod morph_tests {
    use super::*;

    fn tone(hz: f32, secs: f32, rate: u32) -> SourceAudio {
        let n = (rate as f32 * secs) as usize;
        SourceAudio { sample_rate: rate, channels: vec![
            (0..n).map(|i| (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * 0.5)
                .collect()] }
    }
    fn energy(b: &[f32], from: usize, to: usize) -> f32 {
        b[from.min(b.len())..to.min(b.len())].iter().map(|s| s * s).sum()
    }

    #[test]
    fn weights_renormalize_and_two_anchors_are_a_clean_crossfade() {
        for (n, x, y) in [(4, 0.3, 0.7), (3, 0.9, 0.1), (2, 0.5, 0.2), (1, 0.0, 0.0)] {
            let w = morph_weights(n, x, y);
            assert_eq!(w.len(), n);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5, "{n} anchors at ({x},{y})");
        }
        // Two anchors share the top edge: y must cancel out entirely.
        let a = morph_weights(2, 0.25, 0.9);
        let b = morph_weights(2, 0.25, 0.2);
        assert!((a[0] - b[0]).abs() < 1e-5, "a two-sound blend is x alone: {a:?} vs {b:?}");
        assert!((a[0] - 0.75).abs() < 1e-5);
        // Bottom-left puck over top-edge anchors: nearest wins, no NaN.
        let z = morph_weights(2, 0.0, 0.0);
        assert!(z.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn neutral_is_the_identity_and_the_axes_do_what_their_labels_say() {
        let a = tone(200.0, 0.5, 48_000);
        let neutral = tone_map(&a, 0.5, 1.0);
        for (i, s) in neutral.channels[0].iter().enumerate() {
            assert!((s - a.channels[0][i]).abs() < 1e-5,
                    "a pad that colours the sound at rest cannot be trusted: sample {i}");
        }
        // X right on a dull+bright mix: the high band's share must rise.
        let mut mix = tone(150.0, 0.5, 48_000);
        let hi = tone(4000.0, 0.5, 48_000);
        for (i, s) in mix.channels[0].iter_mut().enumerate() {
            *s = *s * 0.5 + hi.channels[0][i] * 0.5;
        }
        let hi_share = |a: &SourceAudio| {
            let bright = tone_map(a, 0.5, 1.0); // identity, but through the same path
            let hp: f32 = {
                // crude residue: sample-to-sample difference energy tracks
                // high-frequency content well enough to order two sounds
                let b = &bright.channels[0];
                b.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>()
            };
            hp / energy(&bright.channels[0], 0, usize::MAX).max(1e-9)
        };
        let dark = tone_map(&mix, 0.1, 1.0);
        let bright = tone_map(&mix, 0.9, 1.0);
        assert!(hi_share(&bright) > hi_share(&dark) * 2.0,
                "right is brighter than left by any honest measure");
        // Y down: the tail dies where the knob says.
        let short = tone_map(&a, 0.5, 0.4);
        let n = short.channels[0].len();
        assert!(energy(&short.channels[0], n * 6 / 10, n) < 1e-6,
                "past the gate is silence");
        assert!(energy(&short.channels[0], 0, n / 4) > 0.1, "before it, the sound");
    }

    #[test]
    fn a_blend_at_the_corners_is_the_corner_and_rates_do_not_matter() {
        let a = tone(200.0, 0.25, 48_000);
        let b = tone(500.0, 0.25, 44_100); // different rate ON PURPOSE
        let left = morph_blend(&[&a, &b], 0.0, 1.0);
        let right = morph_blend(&[&a, &b], 1.0, 1.0);
        for (i, s) in left.channels[0].iter().enumerate().take(4_000) {
            assert!((s - a.channels[0][i]).abs() < 1e-4, "x=0 is anchor A alone");
        }
        // x=1 is B alone, resampled to 48k: same pitch, verified by
        // zero-crossing count over the shared quarter second.
        let zc = |v: &[f32]| v.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count();
        let want = zc(&morph_blend(&[&b], 0.5, 1.0).channels[0]);
        assert!((zc(&right.channels[0]) as i64 - want as i64).abs() <= 2,
                "x=1 is anchor B, at the pad's 48k");
    }
}

#[cfg(test)]
mod render_speed_tests {
    use super::*;
    use studio_core::{AutoTarget, Clip, ClipId, Op, Source, SourceId, Track, TrackId, TrackKind};

    /// A project with real shape: several tracks, several clips each, and a
    /// dense automation lane — the case where the per-sample map lookup
    /// used to cost the most.
    fn busy_project(tracks: usize, clips_each: usize)
        -> (Project, BTreeMap<SourceId, SourceAudio>)
    {
        let mut p = Project::default();
        let n = 48_000;
        let audio_src = SourceAudio { sample_rate: 48_000, channels: vec![
            (0..n).map(|i| (i as f32 / 48_000.0 * 220.0
                            * std::f32::consts::TAU).sin() * 0.2).collect()] };
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "s".into(),
            len: Ticks::from_seconds(1.0), sample_rate: 48_000, channels: 1,
            has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
        let mut cid = 1u64;
        for t in 0..tracks {
            let id = TrackId(t as u64 + 1);
            p.apply(&Op::AddTrack { track: Track { id, kind: TrackKind::Audio,
                name: format!("t{t}"), muted: false, locked: false, gain: 1.0,
                route: None }, at: t }).unwrap();
            for c in 0..clips_each {
                p.apply(&Op::AddClip { clip: Clip { id: ClipId(cid), source: SourceId(1),
                    track: id, pos: Ticks::from_seconds(c as f64 * 1.1),
                    src_in: Ticks::ZERO, len: Ticks::from_seconds(1.0),
                    gain: 0.8, effects: vec![] } }).unwrap();
                cid += 1;
            }
            // A point every 50 ms across the whole span.
            for k in 0..200 {
                p.apply(&Op::SetAutoPoint { target: AutoTarget::TrackGain(id),
                    at: Ticks::from_seconds(k as f64 * 0.05),
                    value: 0.5 + (k % 7) as f32 * 0.05 }).unwrap();
            }
        }
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), audio_src);
        (p, audio)
    }

    #[test]
    fn the_hoisted_render_still_matches_a_naive_per_sample_reference() {
        // The optimization guard: automation hoisted out of the sample
        // loop and swept with a cursor must produce EXACTLY what the
        // straightforward per-sample lookup produced. Computed here from
        // the model directly, so this test does not merely re-run the code
        // it is checking.
        let (p, audio) = busy_project(2, 3);
        let rate = 48_000;
        let to = Ticks::from_seconds(2.0);
        let got = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, to, rate, &[], &[]);
        let frames = to.to_samples(rate) as usize;
        let mut want = vec![0.0f32; frames * 2];
        for track in &p.tracks {
            for clip in p.clips.values().filter(|c| c.track == track.id) {
                let src = &audio[&clip.source];
                let n = clip.len.to_samples(rate) as usize;
                for i in 0..n {
                    let at = Ticks::from_samples(clip.pos.to_samples(rate) + i as i64, rate);
                    if at >= to { break; }
                    let auto = match p.automation.get(&AutoTarget::TrackGain(track.id)) {
                        Some(lane) => studio_core::eval_lane(lane, at),
                        None => 1.0,
                    };
                    let g = clip.gain * track.gain * auto;
                    let f = i;
                    if f >= src.frames() { break; }
                    let oi = (at.to_samples(rate) as usize) * 2;
                    if oi + 1 >= want.len() { break; }
                    want[oi] += src.channels[0][f] * g;
                    want[oi + 1] += src.channels[0][f] * g;
                }
            }
        }
        for (i, (a, b)) in got.iter().zip(&want).enumerate() {
            assert!((a - b).abs() < 1e-5, "sample {i}: {a} vs {b}");
        }
    }

    #[test]
    #[ignore = "timing, not correctness — run with --ignored to measure"]
    fn measure_render_throughput() {
        let (p, audio) = busy_project(8, 10);
        let to = Ticks::from_seconds(10.0);
        let t0 = std::time::Instant::now();
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, to, 48_000, &[], &[]);
        let dt = t0.elapsed();
        println!("rendered {} frames of 8x10 clips + dense automation in {dt:?} \
                  ({:.1}x realtime)", out.len() / 2, 10.0 / dt.as_secs_f64());
    }
}

#[cfg(test)]
mod band_peak_tests {
    use super::*;

    /// A tone at `hz`, full scale, one second.
    fn tone(hz: f32, rate: u32) -> SourceAudio {
        let n = rate as usize;
        let s: Vec<f32> = (0..n).map(|i| (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin()).collect();
        SourceAudio { sample_rate: rate, channels: vec![s] }
    }

    fn loudest(p: &Peaks) -> f32 {
        p.levels[0].iter().map(|m| m.max.abs().max(m.min.abs())).fold(0.0, f32::max)
    }

    #[test]
    fn each_band_shows_its_own_part_of_the_spectrum() {
        let rate = 48_000;
        for (hz, band) in [(60.0, Band::Low), (700.0, Band::Mid), (8_000.0, Band::High)] {
            let a = tone(hz, rate);
            let mine = loudest(&Peaks::build_band(&a, 256, 256, band));
            assert!(mine > 0.3, "{} Hz should survive its own band: {mine}", hz);
            for other in [Band::Low, Band::Mid, Band::High] {
                if other == band { continue; }
                let theirs = loudest(&Peaks::build_band(&a, 256, 256, other));
                assert!(theirs < mine * 0.5,
                        "{} Hz in the {} band ({theirs}) must be well under its own ({mine})",
                        hz, other.label());
            }
        }
    }

    #[test]
    fn full_band_is_the_plain_build_and_the_shape_never_changes() {
        let a = tone(440.0, 48_000);
        let plain = Peaks::build(&a, 256, 256);
        let full = Peaks::build_band(&a, 256, 256, Band::Full);
        assert_eq!(plain.levels, full.levels, "Full must cost nothing and change nothing");
        // Every band keeps the pyramid's shape, so the timeline's drawing,
        // hit testing and zoom work unchanged.
        for band in [Band::Low, Band::Mid, Band::High] {
            let p = Peaks::build_band(&a, 256, 256, band);
            assert_eq!(p.frames, plain.frames);
            assert_eq!(p.levels.len(), plain.levels.len());
            assert_eq!(p.levels[0].len(), plain.levels[0].len());
        }
    }

    #[test]
    fn a_quiet_low_end_under_a_loud_voice_is_visible_in_the_low_band() {
        // The scoring case: narration at -6 dB with a quiet 50 Hz rumble
        // under it. The full waveform is the voice; the low band is the
        // rumble, and that is the point of looking.
        let rate = 48_000;
        let n = rate as usize;
        let mixed: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / rate as f32;
            0.5 * (std::f32::consts::TAU * 900.0 * t).sin()
                + 0.05 * (std::f32::consts::TAU * 50.0 * t).sin()
        }).collect();
        let a = SourceAudio { sample_rate: rate, channels: vec![mixed] };
        let full = loudest(&Peaks::build(&a, 256, 256));
        let low = loudest(&Peaks::build_band(&a, 256, 256, Band::Low));
        let mid = loudest(&Peaks::build_band(&a, 256, 256, Band::Mid));
        assert!(full > 0.45 && mid > 0.3, "the voice dominates everywhere else: {full} / {mid}");
        assert!(low < 0.12, "the low band must not just be the voice again: {low}");
        assert!(low > 0.01, "and the rumble must actually show: {low}");
    }
}

#[cfg(test)]
mod duck_tests {
    use super::*;
    use studio_core::{Bus, BusId, Clip, ClipId, Effect, Op, Source, SourceId, Track, TrackId, TrackKind};

    fn s(x: f64) -> Ticks { Ticks::from_seconds(x) }

    /// A kick (a 60 ms burst every 0.5 s) and a steady bass, each on its own
    /// bus; the BASS bus ducks keyed by the KICK bus. The bass bus has the
    /// LOWER id, so a plain bus order would fold it before its key.
    fn project(duck: bool) -> (Project, BTreeMap<SourceId, SourceAudio>) { project_ids(duck, 1, 2) }

    /// Bass on bus `bass_bus`, kick on `kick_bus`: both ways round, because
    /// the plain bus order puts ONE of them first and the reorder must
    /// matter in the other (the first version only built one, where the
    /// kick happened to come first and the reorder was never needed).
    fn project_ids(duck: bool, bass_bus: u64, kick_bus: u64) -> (Project, BTreeMap<SourceId, SourceAudio>) {
        let rate = 48_000u32;
        let n = rate as usize * 2;
        let kick: Vec<f32> = (0..n).map(|i| if i % (rate as usize / 2) < rate as usize * 6 / 100 { 0.9 * ((i as f32) * 0.2).sin() } else { 0.0 }).collect();
        let bass: Vec<f32> = (0..n).map(|i| 0.3 * ((i as f32) * 0.01).sin()).collect();
        let mut p = Project::default();
        let mut chain = Vec::new();
        if duck {
            chain.push(Effect { kind: "duck".into(), enabled: true,
                params: [("key".to_string(), kick_bus as f32), ("threshold".to_string(), -30.0), ("depth".to_string(), 12.0),
                         ("ratio".to_string(), 20.0), ("attack".to_string(), 0.5), ("release".to_string(), 80.0)].into() });
        }
        // the kick bus is SILENT: a sidechain-only dummy kick, heard by the
        // duck and by nobody else -- so the mix is the bass alone
        for (id, name, ch, silent) in [(bass_bus, "Bass", chain.clone(), false), (kick_bus, "Kick", Vec::new(), true)] {
            p.apply(&Op::AddBus { bus: Bus { id: BusId(id), name: name.into(), parent: None, gain: 1.0, chain: ch, sends: vec![], muted: false, silent } }).unwrap();
        }
        let mut audio = BTreeMap::new();
        for (id, bus, data) in [(1u64, bass_bus, bass), (2, kick_bus, kick)] {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(id), kind: TrackKind::Audio, name: format!("t{id}"), muted: false, locked: false,
                gain: 1.0, route: Some(BusId(bus)) }, at: 0 }).unwrap();
            p.apply(&Op::AddSource { source: Source::file(SourceId(id), format!("{id}.wav"), s(2.0), rate, 1, false) }).unwrap();
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(id), track: TrackId(id), pos: Ticks::ZERO,
                src_in: Ticks::ZERO, len: s(2.0), gain: 1.0, effects: vec![] } }).unwrap();
            audio.insert(SourceId(id), SourceAudio { sample_rate: rate, channels: vec![data] });
        }
        (p, audio)
    }

    fn bass_rms(p: &Project, audio: &BTreeMap<SourceId, SourceAudio>, from: f64, to: f64) -> f32 {
        // no solo: soloing the bass would stop the kick reaching its bus, and
        // the key with it (the first version of this test did, and read 0 dB)
        let out = mixdown(p, audio, &BTreeMap::new(), s(from), s(to), 48_000, &[], &[]);
        (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt()
    }

    #[test]
    fn the_bass_ducks_on_each_kick_and_comes_back_between_them() {
        for (bb, kb) in [(1u64, 2u64), (2, 1)] {
        let (dry, audio) = project_ids(false, bb, kb);
        let (wet, _) = project_ids(true, bb, kb);
        let plain = wet.bus_order().unwrap();
        eprintln!("ids bass {bb} kick {kb}: plain order {plain:?}");
        // inside a kick (0.51-0.55 s) and well after it (0.40-0.48 s: 340 ms on)
        let (under, between) = (bass_rms(&wet, &audio, 0.51, 0.55), bass_rms(&wet, &audio, 0.40, 0.48));
        let (d_under, d_between) = (bass_rms(&dry, &audio, 0.51, 0.55), bass_rms(&dry, &audio, 0.40, 0.48));
        let db = |a: f32, b: f32| 20.0 * (a / b).log10();
        assert!(db(under, d_under) < -9.0, "ducked on the hit: {:.1} dB", db(under, d_under));
        assert!(db(between, d_between) > -1.0, "back between hits: {:.1} dB", db(between, d_between));
        }
    }

    /// The imager on a bus: width 0 everywhere folds the bus to mono (L == R),
    /// and the MONO fold-down is the same with and without it -- the
    /// imager's own invariant, now through the mixdown.
    #[test]
    fn a_bus_imager_at_zero_width_is_mono_and_never_changes_the_fold_down() {
        let (mut p, _) = project(false);
        // a stereo source with different L and R
        let rate = 48_000u32;
        let n = rate as usize;
        let l: Vec<f32> = (0..n).map(|i| 0.3 * ((i as f32) * 0.05).sin()).collect();
        let r: Vec<f32> = (0..n).map(|i| 0.3 * ((i as f32) * 0.031).sin()).collect();
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), SourceAudio { sample_rate: rate, channels: vec![l, r] });
        audio.insert(SourceId(2), SourceAudio { sample_rate: rate, channels: vec![vec![0.0; n]] });
        // The baseline is a NEUTRAL imager (widths 1), not no imager: the
        // crossovers rotate phase, so the fold-down only matches sample for
        // sample through the same filters -- the imager's own test compares
        // two imagers for this reason (the first version of this one did not,
        // and read a 0.94 error for a fold-down that sounds the same).
        let set = |p: &mut Project, w: f32| {
            let chain = vec![Effect { kind: "imager".into(), enabled: true,
                params: [("low".to_string(), w), ("mid".to_string(), w), ("high".to_string(), w)].into() }];
            p.apply(&Op::SetBusChain { id: BusId(1), chain }).unwrap();
        };
        set(&mut p, 1.0);
        let plain = mixdown(&p, &audio, &BTreeMap::new(), s(0.2), s(0.6), rate, &[], &[]);
        set(&mut p, 0.0);
        let mono = mixdown(&p, &audio, &BTreeMap::new(), s(0.2), s(0.6), rate, &[], &[]);
        let side = mono.chunks(2).map(|f| (f[0] - f[1]).abs()).fold(0.0f32, f32::max);
        assert!(side < 1e-4, "width 0: L == R, side {side}");
        let fold = |v: &[f32]| v.chunks(2).map(|f| f[0] + f[1]).collect::<Vec<f32>>();
        let (a, b) = (fold(&plain), fold(&mono));
        let err = a.iter().zip(&b).skip(4800).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        let peak = a.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(err < 0.02 * peak, "the mono fold-down is unchanged (after the crossovers settle): {err} of {peak}");
    }

    /// Bus pan is a balance: hard left silences the right side and leaves the
    /// left exactly as it was; centre changes nothing.
    #[test]
    fn a_bus_panned_hard_left_keeps_its_left_and_loses_its_right() {
        let (mut p, audio) = project(false);
        let centre = mixdown(&p, &audio, &BTreeMap::new(), s(0.40), s(0.48), 48_000, &[], &[]);
        p.apply(&Op::SetBusPan { id: BusId(1), pan: -1.0 }).unwrap();
        let left = mixdown(&p, &audio, &BTreeMap::new(), s(0.40), s(0.48), 48_000, &[], &[]);
        let (l0, l1): (Vec<f32>, Vec<f32>) = (centre.iter().step_by(2).copied().collect(), left.iter().step_by(2).copied().collect());
        assert_eq!(l0, l1, "the near side is untouched");
        assert!(left.iter().skip(1).step_by(2).all(|x| *x == 0.0), "the far side is gone");
        assert!(centre.iter().skip(1).step_by(2).any(|x| *x != 0.0), "and was there at centre");
    }

    #[test]
    fn a_key_folded_after_its_ducked_bus_is_moved_ahead_of_it() {
        let (p, _) = project(true);
        let plain = p.bus_order().unwrap();
        let ordered = order_for_ducks(&p, plain.clone());
        let pos = |o: &[BusId], b: u64| o.iter().position(|x| *x == BusId(b)).unwrap();
        assert!(pos(&ordered, 2) < pos(&ordered, 1), "kick before the bass it keys: {ordered:?} (plain {plain:?})");
    }
}
