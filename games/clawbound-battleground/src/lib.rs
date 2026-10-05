// Clawbound — Battleground · Rust + wgpu Microvoxel Edition
// An arena for the fourteen legendary ancestors of Clawbound,
// with dynamic weather and map events, everything rendered as instanced
// microvoxels.
//
// Layering (headless core → shell):
//   consts / gods / items / weather / map_events / wild / audio(dsp) / input / sim / systems / abilities / ai
//       — pure data + arithmetic, run under `cargo test` with no GPU
//   voxel / models / props / vfx        — mesh + particle generation (CPU)
//   render / scene / ui / app / headless — wgpu, egui, winit shells

pub mod consts;
pub mod gods;
pub mod items;
pub mod voxel;
pub mod models;
pub mod props;
pub mod blockers;
/// The walkable floor of a map carved out of rock, as an exact SDF.
pub mod floor;
/// Which map this thread is playing; the free map functions dispatch on it.
pub mod map;
/// The match opening as a timeline: pan, lanes, spawns, versus card.
pub mod intro;
/// Watching a match: a director, a chase-cam, free flight, all eased.
pub mod spectator;
/// The intro's fanfare, synthesized brass.
pub mod brass;
/// The match's music, synthesized per map when no music file is in the bank.
pub mod score;
/// Loaded clips played in the audio callback: realtime-safe, three buses, ducking.
pub mod mixer;
/// The space sounds happen in: stereo reverb and echo, set by place.
pub mod reverb;
/// Music, sfx and voices loaded from .opus/.wav in the background.
pub mod soundbank;
/// Which sounds a game event asks for; placement; voice gating.
pub mod cues;
/// Every line the game voices, as data: the recording script.
pub mod voicescript;
/// Who the ancestors are: species, age, land, gender, story, music; checked against the game.
pub mod lore;
/// Brawls: body collision, melee identity, area falloff, siphon, winded.
pub mod brawl;
/// Fight decisions (stances), the playbook of team styles, and the arena
/// league that breeds new pages for it.
pub mod tactics;
pub mod playbook;
pub mod arena;
/// Lane minions: what they fight and when they change their mind.
pub mod minionai;
/// Starwell Glacier: the sky map's geometry.
pub mod sky;
/// What stands on Starwell Glacier, and its meshes.
pub mod skyworld;
pub mod skyprops;
/// Cloud ferries across the sky map's chasms.
pub mod ferry;
/// The Moonwell: healing water, the Moonwyrm's claim, and its boons.
pub mod moonwell;
/// The Moonwyrm's flights over the sky map, and the stars it lets fall.
pub mod wyrmflight;
/// The Moonwyrm's 400 u body: where each part of it is, coiled or flying.
pub mod wyrmbody;
/// Floating stone and drifting cloud round the sky map's islands.
pub mod skydrift;
/// Snow that lies, drifts and keeps prints (ultra; presentation only).
pub mod snowpack;
/// Cloud banks and wisps drifting over the sky map, and who they hide.
pub mod skycloud;
/// A clan's camp at each home base: leader's rock, dens, brambles.
pub mod clancamp;
/// The starlight lifts between the sky map's islands and the Aerie.
pub mod lift;
pub mod kit;
/// Hollowstep: the other of him, the terror radius, the heavy tread.
pub mod stalker;
/// Brasshowl: the lamp, the flight from it, the shown-up.
pub mod lantern;
pub mod sentinel;
pub mod maze;
pub mod machine;
pub mod hours;
pub mod clay;
pub mod geartooth;
pub mod emitters;
pub mod picks;
pub mod rules;
pub mod reaction;
pub mod mazeworld;
/// The Curio shelf's passives, worked (items::Passive).
pub mod curios;
/// Levels 1-15: when abilities unlock, what each rank and mastery adds.
pub mod ranks;
/// When and how strongly the sun flares the lens (post.wgsl draws it).
pub mod lensflare;
/// The Drowned Halls: 3 a side on the sea floor, one lane, one jungle hall.
pub mod atlantis;
/// The Drowned Halls' stonework, glass and sea life, as voxel meshes.
pub mod atlantisprops;
/// What stands in, over and around the Drowned Halls.
pub mod atlantisworld;
/// Headless screenshots of the Drowned Halls (`CLAWBOUND_SHOTS=atlantis`).
pub mod atlantisshots;
pub mod mazeshots;
/// The generated preparation documents in dev/ (cue cards, recording sheet, cue sheet).
pub mod devdocs;
/// Character voxels as smooth surfaces with fur lengths, for the ultra preview.
pub mod smoothvox;
/// The announcer: game-state calls, prioritized, never overlapping.
pub mod announcer;
/// The game's sound effects, designed in code (rendered by examples/gen_sfx).
pub mod sfxgen;
/// Twinfang Pass: the mountain canyon map, 15 a side.
pub mod pass;
/// The pass's world: where its rocks, scrub, pines and waterfalls stand.
pub mod passworld;
/// Rough ground: what the jungle costs to cross, where blockers is what it forbids.
pub mod going;
/// The state behind the LAN screen (discovery is `netcode::lan`).
pub mod lanlobby;
/// Input-only lockstep: the rule that lets two machines play one match.
pub mod lockstep;
/// The lockstep wire: redundant inputs, adaptive delay, time dilation, a network simulator.
pub mod netplay;
/// A LAN match: the lockstep session on the den's UDP socket, and the plan the host sends.
pub mod lanmatch;
/// Oriented capsule hit tests for the cats.
pub mod hitbox;
/// The river crossings: fallen logs that can be worked loose and roll.
pub mod logs;
/// The authored centre of the map: the Hollow and the two screened Runs.
pub mod midjungle;
pub mod glades;
/// The surface an AI authors a map through: digests in, refusals in words.
pub mod mapgen;
pub mod anim;
/// Cut a voxel body into posable parts; posed rig -> per-part matrices.
pub mod rigged;
/// What a cat's body does about roots, ferns, rivers, leaps and fights.
pub mod puppet;
/// Where each paw goes down, and keeping it there: stance locks, chosen landings.
pub mod footfall;
/// The aimed leap: onto rocks and logs, chained from perch to perch.
pub mod leap;
pub mod camps;
/// Clan border markers: placed, refreshed on patrol, and lasting days rather
/// than minutes. The half of the scent system wildkit does not cover, because
/// a trail answers "who walked here" and a marker answers "whose ground is
/// this".
/// What the bot TEAM is doing, as opposed to what each bot is doing: one
/// quorum-gated objective the individual brains defer to, with
/// self-preservation outranking it.
/// Graphics tiers for mobile. Measured from the shaders: the post pass's
/// 24-tap bloom ring and 22-tap god rays are the frame, and the 2048² shadow
/// map is second. Low turns bloom to EXACTLY zero, which is what lets the
/// shader skip the ring rather than scale it.
/// What the camera does after a flick's finger leaves the glass: three modes,
/// because whether a lift means "stop" or "let go" is taste rather than a
/// default. Rigid is today's behaviour exactly.
pub mod glide;
pub mod quality;
pub mod teamplay;
pub mod markers;
/// The clock, and what it does to a cat's senses. `wildkit::sim::climate` says
/// what the weather is like at a place; this says what time it is, which is
/// the other half of "LOS vs ears varies by time of day".
pub mod daylight;
/// The fresh-kill economy: caught, carried, piled, eaten before it turns.
/// wildkit's forage modules are a creature feeding itself; nothing there
/// carries prey home, and carrying it home is the meta.
pub mod freshkill;
/// The Dark Forest bleeding into the living one: sight fails, scent persists,
/// hearing lies. A `PerceptParams` modifier, the pattern `daylight` set.
pub mod shadowrealm;
pub mod weather;
pub mod map_events;
pub mod wild;
pub mod bosses;
pub mod disasters;
pub mod audio;
pub mod input;
pub mod gridpick;
pub mod gamepad;
mod touchkit;
pub mod touch;
pub mod sim;
pub mod systems;
pub mod abilities;
pub mod ai;
/// A bot's last look at the ground: skirt small thickets, turn from walls.
pub mod steer;
/// Prepare an ability, see its guide on the ground, fire with the attack button.
pub mod targeting;
pub mod vfx;
/// Per-ancestor elemental motifs thrown on every cast.
pub mod signature;
/// Blast fronts, ground cracks that heal in rain, the fitted shadow box.
pub mod groundfx;
pub mod render;
pub mod scene;
pub mod ui;
/// The shop's decisions as data: offers, refusals, the clan theme, layout.
pub mod shopview;
/// The shop drawn: the Hoard, bark and vine over shopview.
pub mod shopui;
pub mod launch;
pub mod app;
pub mod headless;
/// Headless screenshots of Starwell Glacier (`CLAWBOUND_SHOTS=sky`).
pub mod skyshots;
