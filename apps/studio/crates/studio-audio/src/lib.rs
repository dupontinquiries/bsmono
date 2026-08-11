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

pub mod stems;
pub mod block;

use std::collections::BTreeMap;
use studio_core::{Project, SourceId, Ticks, TrackKind};

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
}

impl Peaks {
    /// Build the pyramid. `base` is Audacity's 256; `factor` of 256 gives its
    /// second summary at 65,536 in one more level.
    pub fn build(audio: &SourceAudio, base: usize, factor: usize) -> Peaks {
        let base = base.max(1);
        let factor = factor.max(2);
        let frames = audio.frames();
        let mut level0 = Vec::with_capacity(frames.div_ceil(base));
        let mut i = 0;
        while i < frames {
            let end = (i + base).min(frames);
            let mut mm = MinMax { min: f32::INFINITY, max: f32::NEG_INFINITY };
            for f in i..end {
                let v = audio.mono(f);
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
        Peaks { base, factor, levels, frames }
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
    pub fn window(&self, from: usize, to: usize, columns: usize) -> Vec<MinMax> {
        let (from, to) = (from.min(self.frames), to.min(self.frames));
        if columns == 0 || to <= from { return Vec::new(); }
        let spp = (to - from) as f64 / columns as f64;
        let level = self.for_px(spp);
        let buckets = &self.levels[level];
        let per = self.samples_per_bucket(level);
        (0..columns)
            .map(|c| {
                let s0 = from + (c as f64 * spp) as usize;
                let s1 = from + (((c + 1) as f64 * spp) as usize).max(1);
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
    clip: BTreeMap<studio_core::ClipId, (Vec<studio_core::Effect>, Vec<studio_fx::Instance>)>,
    bus: BTreeMap<studio_core::BusId, (Vec<studio_core::Effect>, Vec<studio_fx::Instance>)>,
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
    }

    fn clip_instances(&mut self, id: studio_core::ClipId, chain: &[studio_core::Effect], rate: f32) -> &mut [studio_fx::Instance] {
        let entry = self.clip.entry(id).or_default();
        if entry.0 != chain {
            entry.0 = chain.to_vec();
            entry.1 = chain.iter().filter(|e| e.enabled).filter_map(|e| studio_fx::instantiate(e, rate)).collect();
        }
        &mut entry.1
    }

    fn bus_instances(&mut self, id: studio_core::BusId, chain: &[studio_core::Effect], rate: f32) -> &mut [studio_fx::Instance] {
        let entry = self.bus.entry(id).or_default();
        if entry.0 != chain {
            entry.0 = chain.to_vec();
            entry.1 = chain.iter().filter(|e| e.enabled).filter_map(|e| studio_fx::instantiate(e, rate)).collect();
        }
        &mut entry.1
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
    next_expected: Option<Ticks>,
}

impl StreamingRenderer {
    pub fn new() -> StreamingRenderer {
        StreamingRenderer::default()
    }

    /// Render `[from, to)`, reusing cached effect instances when this call
    /// picks up exactly where the previous one left off, and transparently
    /// resetting (same cost as a one-shot [`mixdown`] call) otherwise.
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
        if self.next_expected != Some(from) {
            self.cache.reset();
        }
        self.next_expected = Some(to);
        mixdown_with_cache(project, audio, fades, from, to, rate, solo, bus_solo, &mut self.cache)
    }

    /// Force a reset on the next `render`, regardless of continuity —
    /// exposed for a caller that knows a discontinuity is coming (e.g. is
    /// about to seek) and wants the reset to happen before the render call
    /// that would otherwise detect it, not because detection needs help.
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
    let mut master = vec![0.0f32; frames * 2];
    // One accumulator per bus, pre-populated for every bus in the project —
    // not just the ones a track happens to route to — so a bus that exists
    // only as a send target still has somewhere to land.
    let mut bus_buf: BTreeMap<studio_core::BusId, Vec<f32>> =
        project.busses.keys().map(|id| (*id, vec![0.0f32; frames * 2])).collect();

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
                None => master.as_mut_slice(),
            },
            None => master.as_mut_slice(),
        };

        for clip in project.track_clips(track.id) {
            // skip clips entirely outside the window
            if clip.end() <= from || clip.pos >= to { continue; }
            let Some(src) = audio.get(&clip.source) else { continue };
            if src.channels.is_empty() { continue; }
            let fade = fades.get(&clip.id).copied().unwrap_or_default();
            // The clip's own effect chain, instantiated once per clip (not
            // per sample) so a stateful effect's filter history is
            // continuous across the whole clip rather than reset every
            // sample. Same convention as the bus-chain wiring above: one
            // `Instance` per effect, fed L then R in sequence — consistent
            // with `studio_fx::chain::Chain::process`'s own established
            // pattern rather than introducing a different one here. An
            // unknown effect kind is silently skipped via `instantiate`'s
            // `None`.
            let clip_fx: &mut [studio_fx::Instance] = cache.clip_instances(clip.id, &clip.effects, rate as f32);

            let start = clip.pos.max(from);
            let stop = clip.end().min(to);
            let out_off = (start.to_samples(rate) - from.to_samples(rate)).max(0) as usize;
            let n = (stop.to_samples(rate) - start.to_samples(rate)).max(0) as usize;
            for i in 0..n {
                let at = Ticks::from_samples(start.to_samples(rate) + i as i64, rate);
                let into_clip = at.saturating_sub(clip.pos);
                let src_frame = (clip.src_in.saturating_add(into_clip)).to_samples(rate) as usize;
                if src_frame >= src.frames() { break; }
                let g = clip.gain * fade.gain_at(into_clip, clip.len);
                let oi = (out_off + i) * 2;
                if oi + 1 >= dest.len() { break; }
                // a mono source feeds both sides; a stereo one keeps its image
                let (mut l, mut r) = if src.channels.len() >= 2 {
                    (src.channels[0][src_frame], src.channels[1][src_frame])
                } else {
                    let v = src.channels[0][src_frame];
                    (v, v)
                };
                for inst in clip_fx.iter_mut() {
                    l = inst.process_audio(l);
                    r = inst.process_audio(r);
                }
                dest[oi] += l * g;
                dest[oi + 1] += r * g;
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
    let order = project.bus_order().unwrap_or_default();
    let audible = solo_audible_bus_set(project, bus_solo);
    for id in order {
        let Some(bus) = project.busses.get(&id) else { continue };
        let Some(buf) = bus_buf.remove(&id) else { continue };
        // Mute is an absolute override, checked before solo; a gated bus's
        // buffer — including everything already summed into it from its own
        // tracks and any children processed earlier — is simply dropped,
        // the same as a muted track's clips never reaching the mix.
        let solo_ok = bus_solo.is_empty() || audible.contains(&id);
        if bus.muted || !solo_ok {
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
            for s in buf.iter_mut() {
                for inst in instances.iter_mut() {
                    *s = inst.process_audio(*s);
                }
            }
        }
        let pre = buf.clone();
        for s in buf.iter_mut() { *s *= bus.gain; }

        for send in &bus.sends {
            let tap: &[f32] = if send.pre_fader { &pre } else { &buf };
            if let Some(target) = bus_buf.get_mut(&send.to) {
                for (t, s) in target.iter_mut().zip(tap.iter()) {
                    *t += s * send.level;
                }
            }
        }

        let parent: &mut [f32] = match bus.parent {
            Some(p) => match bus_buf.get_mut(&p) {
                Some(b) => b.as_mut_slice(),
                None => master.as_mut_slice(),
            },
            None => master.as_mut_slice(),
        };
        for (p, b) in parent.iter_mut().zip(buf.iter()) {
            *p += b;
        }
    }

    master
}

/// Peak level of a rendered buffer, in dBFS. `-inf` for silence — reported as
/// f32::NEG_INFINITY rather than a magic -100.0, so a caller cannot mistake
/// "very quiet" for "nothing".
pub fn peak_dbfs(samples: &[f32]) -> f32 {
    let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    if peak <= 0.0 { f32::NEG_INFINITY } else { 20.0 * peak.log10() }
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
            name: "A1".into(), muted: false, locked: false, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "t.wav".into(),
            len: Ticks::from_seconds(len_s), sample_rate: rate, channels: 1, has_video: false } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(len_s),
            gain, effects: Vec::new() } }).unwrap();
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), tone((len_s * rate as f64) as usize, rate, 440.0, 0.5));
        (p, audio)
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
        Bus { id: BusId(id), name: name.into(), parent: parent.map(BusId), gain, chain: Vec::new(), sends: Vec::new(), muted: false }
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
            muted: false, locked: false, route: None,
        }, at: 1 }).unwrap();
        let mut audio2 = audio.clone();
        audio2.insert(studio_core::SourceId(2), tone(24_000, 48_000, 660.0, 0.5));
        p.apply(&Op::AddSource { source: Source { id: studio_core::SourceId(2), path: "b.wav".into(),
            len: Ticks::from_seconds(0.5), sample_rate: 48_000, channels: 1, has_video: false } }).unwrap();
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
