//! Prompt engineering for the paintbrush.
//!
//! The question "when do I add detail versus start from a palette" has
//! an answer builders already use, and the narration corpus made it
//! measurable: they work in PASSES. Massing first (the big shapes, in
//! one cheap material), then materials (restyle to a palette), then
//! detail (windows, paths, scatter). Detail on an unfinished mass is
//! wasted; a palette on a shape you will change is wasted.
//!
//! So the prompt does not ask the model to build a finished thing. It
//! asks for a pass, names which pass, and gives the model the previous
//! pass's result as the thing to work on. "Expand this structure" is
//! then not a special case -- it is the detail pass with a referent.
//!
//! Everything in the system prompt is stable across turns. Put it
//! first and cache the prefix; only the user turn changes.

/// Which pass the model is being asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Big shapes in one cheap material. Few ops, large ones.
    Massing,
    /// Restyle the massing to a named palette. Mostly `restyle`.
    Materials,
    /// Windows, doors, paths, trim, scatter. Many small ops.
    Detail,
    /// Make it bigger / add a wing / extend -- a massing pass anchored
    /// on an existing structure.
    Expand,
}

impl Pass {
    pub fn name(self) -> &'static str {
        match self { Pass::Massing => "massing", Pass::Materials => "materials",
                     Pass::Detail => "detail", Pass::Expand => "expand" }
    }

    /// Infer the pass from the user's words when they do not name one.
    /// Conservative: unknown phrasing defaults to Massing, which is the
    /// safe first pass on an empty site, or Detail when a structure
    /// already exists at the focus.
    pub fn infer(prose: &str, has_structure: bool) -> Pass {
        let p = prose.to_lowercase();
        let any = |ws: &[&str]| ws.iter().any(|w| p.contains(w));
        if any(&["bigger", "expand", "extend", "add a wing", "taller", "wider", "more of"]) {
            Pass::Expand
        } else if any(&["make it look", "restyle", "material", "palette", "out of ", "in stone",
                        "in wood", "jungle", "volcanic", "snowy"]) && has_structure {
            Pass::Materials
        } else if any(&["window", "door", "path", "decorat", "detail", "roof", "trim",
                        "lantern", "flower", "garden", "add "]) && has_structure {
            Pass::Detail
        } else if has_structure {
            Pass::Detail
        } else {
            Pass::Massing
        }
    }
}

/// Rough token count. Four characters per token is the usual English
/// approximation and is close enough for budgeting; the point is to
/// notice a prompt that will not fit, not to be exact.
pub fn approx_tokens(s: &str) -> usize { s.len() / 4 + 1 }

/// How much room the prompt may take, given the model's context and the
/// reply budget. Leaves headroom: a prompt that exactly fills the
/// context leaves no room to generate, and the model emits nothing or
/// truncates mid-line.
pub fn prompt_budget_tokens(ctx: usize, max_reply: usize) -> usize {
    ctx.saturating_sub(max_reply).saturating_sub(128).max(256)
}

/// A much shorter system prompt, for small models and tight contexts.
///
/// A 1B handed the full 3.3k-character prompt continued it by REPEATING
/// IT — eight lines of the OPS reference came back as a "plan". More
/// instruction is not more compliance: past some length a small model
/// treats the reference as the thing to imitate. This keeps one worked
/// example and the four ops that matter, and drops the discussion.
pub const COMPACT: &str = "\
Reply with plan lines only. No prose, no code fences, no explanation.
@dx,dz is relative to where the player is looking. Y is automatic.

These are real lines. Copy their SHAPE with your own numbers:
terraform @-5,-5 @5,5 smooth block=stone
cylinder @0,0 r=4 h=10 block=stone_bricks hollow
path @4,0 @16,9 w=2 block=cobblestone
tree @8,3 oak h=7
sphere @3,0 r=1 block=air
enclose @-5,-5 @5,5 h=5 block=oak_planks
roof @-5,-5 @5,5 gable block=dark_oak_planks

Blocks: stone, cobblestone, stone_bricks, oak_planks, dark_oak_planks, glass, snow.
A tower 8 blocks wide is r=4, not r=8.

Example -- player: a small watchtower
# watchtower
terraform @-5,-5 @5,5 smooth block=stone
cylinder @0,0 r=3 h=11 block=stone_bricks hollow
roof @-3,-3 @3,3 hip block=dark_oak_planks

A building needs a roof. Add one.

Now reply with plan lines only.";

/// Below this context, use the compact prompt. A 1B with a 2k window
/// given the full 3.3k-character prompt echoed the reference back
/// instead of answering; the fix is less prompt, not more.
pub const COMPACT_BELOW_CTX: usize = 8192;

/// Pick a system prompt that fits. `ctx` is the model's context window
/// in tokens; `max_reply` the generation budget.
pub fn system_prompt_for(ctx: usize, max_reply: usize) -> String {
    // Two reasons to go compact, and the first is not about arithmetic:
    // a small model drowns in a long prompt regardless of whether it
    // technically fits. Context size is the best proxy available for
    // "small model" without asking the engine about parameter count.
    if ctx < COMPACT_BELOW_CTX { return COMPACT.to_string(); }
    let full = system_prompt();
    let budget = prompt_budget_tokens(ctx, max_reply);
    // The per-turn context (region digest, focus line, prose) is roughly
    // 300 tokens on top of the system prompt.
    if approx_tokens(&full) + 300 <= budget { full } else { COMPACT.to_string() }
}

/// The stable system prompt. Contains the grammar, the pass discipline,
/// and worked examples that are themselves valid IR (there is a test
/// that parses every example line).
pub fn system_prompt() -> String {
    // The world-change rule, the mood list and the form list are all
    // GENERATED from their tables, so the prompt cannot drift from what
    // the parser actually accepts.
    format!("{GRAMMAR}\n\nFORMS (use `form NAME A B`):\n{}\n\n{}\n\n{DISCIPLINE}\n\n{}\n\n{EXAMPLES}",
            crate::dict::vocabulary(), crate::mood::vocabulary(),
            crate::intent::WORLD_DISCIPLINE)
}

pub const GRAMMAR: &str = "\
You build voxel structures by replying with a PLAN: one operation per line, nothing else.
Coordinates are X,Z pairs. Y is found from the ground automatically; never guess it.
@dx,dz means relative to the FOCUS anchor the player is looking at. Prefer @ coordinates.
@ is relative on EVERY axis including Y: `@0,60,0` is sixty blocks UP, not y=60.
Use bare numbers when you mean an absolute height.
Blocks are names (stone, cobble, planks, mossy, glass) or palette roles (jungle.primary).

OPS
  terraform A B [delta=N] [smooth] [block=B]     raise/lower/smooth ground in the A..B footprint
  path A B [w=N] [block=B]                       ground-following path from A to B
  enclose A B [h=N] [block=B]                    wall around footprint A..B
  cylinder A [r=N] [h=N] [block=B] [hollow]      tower; use hollow for rooms
  sphere A [r=N] [block=B] [hollow]              dome or boulder; block=air carves
  tree A SPECIES [h=N] [lean=dx,dz]              oak, birch, pine, cactus
  scatter A B [block=B] [clusters=N] [per=N] [spread=N] [seed=N]
  roof A B [gable|hip|flat|pyramid] [block=B]    pitched roof over a footprint; sits on the walls
  floor A B [y=N] [block=B]                      a storey slab inside a footprint
  bridge A B [w=N] [block=B]                     span at constant height WITH PIERS (path digs in)
  stairs A B [w=N] [block=B]                     rise one block per step from A to B
  mirror A B [x|z] [about=N]                     reflect a region: pillars either side of something
  form NAME A B [style] [k=v]                    a named form from the dictionary
                                                 (village = several houses, not one)
  brush OP A [by=N|to=Y] [r=N] [hill|mesa|cone|flat]   LANDSCAPE: raise, lower, smooth,
                                                 flatten, roughen the ground under a radius
  layer PACKET A B [density=0.6] [radial|gentle]  paint a region: temperate, alpine, boreal,
                                                 jungle, desert, darkwood, meadow, barren
  road A B [w=N] [block=B]                       a road that cuts, fills, bridges and tunnels
                                                 as the ground demands; use it between builds
  river A [dir=DEG] [len=N]                      a river from a source; the terrain decides
                                                 the path, pools become lakes with outlets
  lake A r=N level=Y                             flood a basin to a level
  mood NAME A B                                  set the FEEL of a region: palette, decay, clutter,
                                                 and fog/light/time for the engine (see MOODS)
  populate KIND A B [count=N] [name=X]           ask the engine for creatures or NPCs; KIND is
                                                 plain words, the engine picks from its roster
  weather A B [worn|abandoned|ancient|burnt]     age an EXISTING build: decay, moss, rubble
  greeble A B [words...] [gens=N]                evolve surface detail for a region to match
                                                 the words: industrial, gothic, clean, busy,
                                                 symmetric, rustic (A, B are x,y,z)
  detail KIND A B [block=B] [amount=0.3]         finishing pass over an existing build:
                                                 speckle (break up a flat face), trim (accent
                                                 the edges), relief (give a wall depth),
                                                 feather (dissolve one material into another),
                                                 blend (spill material around a base)
  views A [depth=N] then front:/side:/top: drawings, then `end`
                                                 DRAW the shape; two views are intersected
  restyle A B FROM TO                            re-theme a box between palettes (A, B are x,y,z)
  fill A B [block=B] [hollow]                    box between x,y,z corners; hollow for rooms

PALETTES  stone oak desert volcanic polished frozen cavern jungle plains snowy mesa darkwood ruined
ROLES     primary secondary accent foliage

Sizes are BLOCKS. r=4 makes a tower 9 across. A 10-wide tower is r=4, not r=10.
Nothing over 200000 blocks. If unsure, make it smaller; the player can say bigger.";

pub const DISCIPLINE: &str = "\
BUILD IN PASSES. You will be told which pass this is.

  massing    The big shapes only, in ONE cheap material (stone or planks). Few ops, large.
             No windows, no paths, no scatter. Get the silhouette right; everything else waits.
  materials  Change what the massing is made of. Mostly restyle over the structure's box.
             Do not change shape. Do not add detail.
  detail     Now the small things: A ROOF if the building has none, then a finishing pass:
             `detail speckle` on any large flat face and `detail trim` on the edges are worth
             doing on almost every build. Doors as carved air,
             windows, floors for upper storeys, a path to the entrance,
             trim with the accent role, scatter foliage nearby. Many small ops.
             Do not touch the massing.
  expand     A massing pass on an existing structure: extend it, add a wing, raise it.
             Match its material. Anchor on the structure, not on the player's feet.

READ FIRST. The context tells you what is at the focus and what was built last.
If there is a LAST BUILD, your ops should relate to it -- attach, surround, extend.
If the focus tier is 'detail' and you were asked for something large, say so in one line
before the plan; the player may need to /focus bigger.

If a shape is easier to DRAW than to describe, use `views`: give a front
and a side elevation in ASCII (`#` solid, `.` empty) and they will be
intersected. Two views cannot express a hidden concavity -- add `top:`
for anything with a notch. You will be told if the reconstruction did
not match what you drew.

Reply with the plan only. No prose, no explanation, no code fences.";

pub const EXAMPLES: &str = "\
EXAMPLE (massing, empty site, focus tier structure)
  player: a small watchtower on this hill
  plan:
terraform @-5,-5 @5,5 smooth block=stone
cylinder @0,0 r=3 h=11 block=stone hollow
roof @-4,-4 @4,4 hip block=dark_oak_planks

EXAMPLE (materials, LAST BUILD = the tower above)
  player: make it look old and mossy
  plan:
restyle @-4,-1,-4 @4,15,4 stone ruined

EXAMPLE (detail, LAST BUILD = the tower above)
  player: add a door and a path down to the road
  plan:
sphere @3,0 r=1 block=air
path @3,0 @18,12 w=2 block=cobble
scatter @-8,-8 @8,8 block=flower clusters=3 per=5 spread=2 seed=11

EXAMPLE (massing, empty site; the request names a MOOD and CREATURES)
  player: build an eerie forest with horrifying creatures
  plan:
# eerie forest
tree @-14,-12 oak h=9
tree @-6,-15 oak h=11
tree @4,-10 oak h=8
tree @12,-14 oak h=10
tree @-10,2 oak h=12
tree @8,6 oak h=9
tree @-2,12 oak h=10
tree @14,10 oak h=8
mood eerie @-18,-18 @18,18
populate horrifying creatures @-18,-18 @18,18 count=6

EXAMPLE (LANDSCAPE, hundreds of thousands of blocks from a few lines)
  player: a valley with a forested range on one side and a river down it
  plan:
# valley
brush raise @-60,0 by=30 r=70 hill
brush raise @70,0 by=18 r=50 mesa
brush roughen @0,0 by=3 r=120 flat
layer boreal @-120,-60 @0,60 density=0.7 gentle
layer meadow @0,-40 @60,40 density=0.4
river @-50,10 dir=0 len=400

EXAMPLE (massing; a lair for a named boss)
  player: build a fire dragon nest
  plan:
# dragon nest
form nest @-12,-12 @12,12 depth=6 rim=3 block=basalt
mood fiery @-16,-16 @16,16
populate fire dragon @-6,-6 @6,6 count=1

EXAMPLE (expand, LAST BUILD = the tower above)
  player: taller, and give it a base
  plan:
cylinder @0,0 r=5 h=3 block=stone
cylinder @0,0 r=3 h=6 y=76 block=stone hollow";

/// The OUTLINE pass: decide what to build, in words, before writing a
/// single coordinate.
///
/// Splitting the model's work in two helps a small model for the same
/// reason it helps a person: choosing what to build and expressing it
/// in a formal notation are different jobs, and doing both at once is
/// where a 1B drops one of them. The outline is short prose, so there
/// is no grammar to get wrong; the worker pass has a decision already
/// made, so it only has to translate.
///
/// It costs a second round trip. Off by default for that reason —
/// worth it for "a village" and wasteful for "a tower".
pub const OUTLINE: &str = "\
You are planning a build. Do NOT write coordinates or commands yet.

Read the terrain and site notes. Then answer in at most five short
lines, each naming ONE thing to build and roughly where and how big:

  a stone watchtower on the high ground east, about 7 across, 12 tall
  a path from its door down to the level ground west
  three pines scattered on the slope below

Rules:
- Respect the terrain. Do not put a large flat footprint on a steep
  slope; either pick the level ground or say the ground must be
  terraformed first.
- Use the site palette unless the player asked otherwise.
- Fewer, larger things beat many small ones.

Answer with the lines only.";

/// Turn an outline into the worker turn.
pub fn worker_turn(context: &str, outline: &str, prose: &str) -> String {
    format!("{context}\n\nThe plan you already decided on:\n{outline}\n\n\
             player: {prose}\nNow write it as plan lines. plan:")
}

/// Per-turn user message: context first (so the model reads before it
/// writes), then the pass, then the request.
pub fn user_turn(context: &str, pass: Pass, prose: &str) -> String {
    format!("{context}\n\nPASS: {}\nplayer: {prose}\nplan:", pass.name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parse_plan_at;

    #[test]
    fn every_example_plan_line_is_valid_ir() {
        // The examples teach the grammar; a broken example teaches a
        // broken grammar. Extract each `plan:` block and parse it with
        // an anchor so @ coordinates resolve.
        let mut n = 0;
        for block in EXAMPLES.split("plan:").skip(1) {
            let lines: Vec<&str> = block.lines()
                .skip_while(|l| l.trim().is_empty())
                .take_while(|l| !l.trim_start().starts_with("EXAMPLE") && !l.trim().is_empty())
                .collect();
            let src = lines.join("\n");
            let (plan, issues) = parse_plan_at(&src, Some((100, 65, 100)));
            assert!(issues.is_empty(), "example failed to parse:\n{src}\n{issues:?}");
            assert!(!plan.ops.is_empty(), "example produced no ops:\n{src}");
            n += 1;
        }
        assert_eq!(n, 7, "expected seven worked examples");
    }

    #[test]
    fn pass_inference_follows_the_discipline() {
        assert_eq!(Pass::infer("a small stone house", false), Pass::Massing);
        assert_eq!(Pass::infer("a small stone house", true), Pass::Detail,
                   "with a structure present, unspecified requests default to detail");
        assert_eq!(Pass::infer("make it look volcanic", true), Pass::Materials);
        assert_eq!(Pass::infer("add windows and a door", true), Pass::Detail);
        assert_eq!(Pass::infer("make it taller", true), Pass::Expand);
        assert_eq!(Pass::infer("expand this structure", false), Pass::Expand);
    }

    #[test]
    fn system_prompt_is_stable_and_self_contained() {
        let a = system_prompt();
        let b = system_prompt();
        assert_eq!(a, b, "must be byte-identical across calls for prefix caching");
        for needle in ["terraform", "restyle", "@dx,dz", "r=4 makes a tower 9 across",
                       "BUILD IN PASSES", "READ FIRST"] {
            assert!(a.contains(needle), "missing {needle}");
        }
        assert!(a.is_ascii(), "keep the prompt ASCII for tokenizer portability");
    }

    #[test]
    fn user_turn_puts_context_before_the_request() {
        let t = user_turn("FOCUS f1 | looking at grass", Pass::Massing, "a hut");
        let ctx_at = t.find("FOCUS").unwrap();
        let req_at = t.find("player: a hut").unwrap();
        assert!(ctx_at < req_at);
        assert!(t.ends_with("plan:"), "ends where the model should start writing");
    }

    #[test]
    fn a_small_context_gets_the_compact_prompt() {
        // The 1B in the field: ~1000 tokens of prompt into a small ctx,
        // 512 of reply, and it echoed the reference back.
        let small = system_prompt_for(2048, 512);
        assert_eq!(small, COMPACT, "2k ctx must not get the full prompt");
        let big = system_prompt_for(16384, 512);
        assert_eq!(big, system_prompt(), "16k ctx has room for the full one");
        assert!(approx_tokens(&small) < approx_tokens(&big) / 2,
                "compact must be much smaller: {} vs {}",
                approx_tokens(&small), approx_tokens(&big));
    }

    #[test]
    fn the_budget_always_leaves_room_to_generate() {
        // a prompt that exactly fills the context leaves nothing to say
        assert!(prompt_budget_tokens(2048, 512) <= 2048 - 512);
        assert!(prompt_budget_tokens(512, 512) >= 256, "degenerate input stays usable");
        assert!(prompt_budget_tokens(0, 0) >= 256);
    }

    #[test]
    fn the_compact_examples_are_valid_ir_too() {
        use crate::plan::parse_plan_at;
        let plan_lines: Vec<&str> = COMPACT.lines()
            .skip_while(|l| !l.starts_with("Example"))
            .skip(1)
            .take_while(|l| !l.trim().is_empty())
            .collect();
        assert!(plan_lines.len() >= 3, "{plan_lines:?}");
        let (plan, issues) = parse_plan_at(&plan_lines.join("\n"), Some((0, 65, 0)));
        assert!(issues.is_empty(), "compact example must parse: {issues:?}");
        assert_eq!(plan.name, "watchtower");
    }

    #[test]
    fn the_compact_prompt_carries_no_placeholder_lines() {
        // It must not itself be echo-bait: if the model copies these
        // lines they should still be valid IR-shaped, not `A B [x=N]`.
        for l in COMPACT.lines() {
            let t = l.trim();
            if crate::llm::KNOWN_VERBS.iter().any(|v| t.starts_with(v)) {
                assert!(!t.contains('[') && !t.contains(']'),
                        "no bracket notation in the compact prompt: {t}");
            }
        }
    }

    #[test]
    fn every_verb_line_in_the_compact_prompt_is_itself_valid_ir() {
        // The reference lines are echo-bait by nature -- a small model
        // copies them. So they must be REAL: if the model returns one
        // verbatim, it should build something, not fail to parse. The
        // first version used @X,Z and r=RADIUS, which would have failed
        // exactly the way the full prompt's `A B [r=N]` did.
        use crate::plan::parse_plan_at;
        let mut checked = 0;
        for l in COMPACT.lines() {
            let t = l.trim();
            if !crate::llm::KNOWN_VERBS.iter().any(|v| t.starts_with(&format!("{v} "))) {
                continue;
            }
            assert!(!crate::llm::is_grammar_echo(t), "echo-shaped: {t}");
            let (plan, issues) = parse_plan_at(t, Some((0, 65, 0)));
            assert!(issues.is_empty() && plan.ops.len() == 1,
                    "compact reference line must parse: {t:?} -> {issues:?}");
            checked += 1;
        }
        assert!(checked >= 5, "checked only {checked} reference lines");
    }

    #[test]
    fn the_outline_pass_forbids_coordinates() {
        // Its whole purpose is to make the decision without the
        // notation; if it starts emitting IR the split has bought
        // nothing.
        assert!(OUTLINE.contains("Do NOT write coordinates"));
        assert!(!OUTLINE.contains('@'), "no coordinate syntax to imitate");
        assert!(OUTLINE.is_ascii());
    }

    #[test]
    fn the_worker_turn_carries_the_decision_forward() {
        let t = worker_turn("SITE: temperate", "a stone tower on the rise", "a tower");
        assert!(t.contains("SITE: temperate"), "context survives");
        assert!(t.contains("a stone tower on the rise"), "and the outline");
        assert!(t.ends_with("plan:"), "ending where the model should start writing");
    }
}
