#include "lore.h"
#include <stddef.h>

/* Original codex writing themed on the six-film arc. No source text is
   reproduced; these are our own summaries of the concepts. */
static const ex_lore_entry_t ENTRIES[] = {
  { "apex: resonance", {
    "the devourer keeps a rhythm. every nine seconds a pulse:",
    "a field that freezes the joints, a hurled ruin, a fan of",
    "fire, a well that drags you in. learn the count and live.",
  }, 3 },
  { "I. doomsday", {
    "a sleeping devourer stirs beneath the world.",
    "the first war is not for territory but for whether",
    "anyone survives to fight the next one.",
  }, 3 },
  { "II. dominion", {
    "the tyrant does not want to rule you.",
    "he wants you to believe you chose him.",
    "order offered as a cage with the door left open.",
  }, 3 },
  { "III. destiny", {
    "a leader becomes what the moment demands,",
    "and pays for it. the door to the wider cosmos opens.",
    "coordinates arrive from a hand that has waited an age.",
  }, 3 },
  { "IV. first light", {
    "outside the shield the rules are different.",
    "wonder and dread share the same dark.",
    "the watch bearer trusts no one yet -- and must anyway.",
  }, 3 },
  { "V. solar", {
    "a fire clone holds a star in his palm,",
    "and asks why you lead as if apologizing for it.",
    "some weapons are too large to aim.",
  }, 3 },
  { "VI. photon", {
    "the watch has a ceiling no one has found.",
    "for forty seconds once, it became light itself.",
    "the message home is simple: it is beautiful out here.",
  }, 3 },
  { "the watch", {
    "eleven reliable forms, and one that is not.",
    "found, not made, at the last place the makers worked.",
    "it answers to one bearer, and to no other.",
  }, 3 },
  { "the corrupted", {
    "unicron's tissue hijacks the will to transform.",
    "no fixed shape, ever. they hunt by spark resonance,",
    "so stillness will not hide you. only the sparkless walk free.",
  }, 3 },
  { "the hijacks", {
    "they do not conquer. they gather data.",
    "erratic, endless, indifferent to their own losses --",
    "sending fifty where three would do, and not caring.",
  }, 3 },
  { "the gravity leap", {
    "an ice clone folds the cold and steps across light-years.",
    "no flash, no sound -- only a wave that arrives after,",
    "indifferent to who is standing where it lands.",
  }, 3 },
  { "the array", {
    "a rotating crystal sphere aboard the enemy flagship.",
    "it watched a world's mantle for decades, then cracked",
    "the day the old legends turned out to be true.",
  }, 3 },
  { "the boundary", {
    "at the edge of the system, a lattice woven from light.",
    "too faint to name on the first pass. flagged on the second.",
    "old enough that even a devourer's echo turns toward it.",
  }, 3 },
  { "the deep outpost", {
    "they descended in the dark and split up -- wrong choice.",
    "a comrade stood still, facing away, and turned too far.",
    "the thing below was slow. the thing above was already up.",
  }, 3 },
  { "the morpher", {
    "the ice-clone watch bearer, glimpsed at the boundary.",
    "one watch, every elemental form -- and in cold form,",
    "the quantum leap no other clone can perform.",
  }, 3 },
  { "billy, the fire clone", {
    "he holds the power of a star in his open hand.",
    "absorb one; fire it. a last resort even the clones",
    "are careful when to spend -- the cost is cosmic.",
  }, 3 },
  { "photon", {
    "not an element the clone races ever had.",
    "the one watch form that transcends them all --",
    "solid light, and a ceiling no one has found.",
  }, 3 },
  { "the reclaim", {
    "an overgrown world of organic-metal corridors.",
    "three long lanes run its length, and slow gun-plinths",
    "sweep them -- outrun their aim or die in the open.",
  }, 3 },
  { "the corridors", {
    "supply ships once ran these lanes at full burn.",
    "interceptor raiders learned to strike from the jungle,",
    "where the shear-corridor funnels everything into a kill-box.",
  }, 3 },
  { "jhiaxus", {
    "the signal architect. never afraid, never cruel --",
    "he calls the horror a data point and revises the model.",
    "the thing he grew does not recognize its own maker.",
  }, 3 },
  { "blackstar", {
    "right about the danger. wrong about the solution.",
    "he is not redeemed and he is not defeated --",
    "he is left holding both truths at once, and walks away.",
  }, 3 },
  { "optimus", {
    "he stops believing himself eternal, and begins",
    "believing in consequence. not a savior -- a steward,",
    "whose last duty is to leave his heirs stronger than he.",
  }, 3 },
  { "rodimus", {
    "the heir who leads as if apologizing for it,",
    "until a fire clone asks him who taught him that.",
    "he follows the coordinates out past the boundary.",
  }, 3 },
  { "aiden", {
    "first human to speak for earth beyond the shield.",
    "five films of disaster behind him, one line home:",
    "it is beautiful out here. you would not believe it.",
  }, 3 },
  { "unicron", {
    "not a villain -- an event. a devourer the matrix",
    "drove dormant but never silenced. his neural remnants",
    "still lace the mantle and ocean floor, waiting to be found.",
  }, 3 },
  { "the technocrats", {
    "archivists, navigators, economists of energy --",
    "a council that built a thing it did not understand,",
    "sealed the blackstar data, and called it coincidence.",
  }, 3 },
  { "cybertron", {
    "it stood for millions of cycles, and mistook",
    "endurance for destiny. rule by restricted data tier;",
    "only the prime-line held the oldest secret of all.",
  }, 3 },
  { "the clone coalition", {
    "elemental nations older than cybertron's golden age,",
    "each a people of a single power -- ice, fire, metal --",
    "holding the shield's perimeter against the hijacks.",
  }, 3 },
  { "the helion alliance", {
    "humans who have lived inside the shield the longest,",
    "and so read its interior as no cybertronian can.",
    "the bearer trusts them a shade more than the rest.",
  }, 3 },
  { "earth", {
    "warm gold and noise and life -- pluto's opposite.",
    "beneath the crust, a metallic hand sinking, geological,",
    "and the sealed truth the prime-line always carried.",
  }, 3 },
  { "the watch forms", {
    "ice, water, tsunami. fire, magma, blizzard.",
    "metal, titanium, electricity. wind, tornado --",
    "eleven mapped. then the door with no handle: light.",
  }, 3 },
  { "the matter planet", {
    "the last place the makers worked -- where the watch",
    "was found, not made. it answers to one bearer alone,",
    "and holds a form even he cannot choose to become.",
  }, 3 },
  { "cryotek", {
    "the cold open: an ice-base decepticon holding the outer rim,",
    "ice-plasma and frozen shards, mining cybertron's dead fragments.",
    "sam ends him early -- the first skirmish, and a statement of tone.",
  }, 3 },
  { "hot rod", {
    "a reckless young scout in earth orbit who idolizes prime.",
    "he dives the ruptured mantle, and at the worst moment",
    "takes up the matrix -- and rises out of it as rodimus.",
  }, 3 },
  { "megatron", {
    "drawn to the energon pulse, he returns changed --",
    "hardened, reflective. he has watched the devourer eat",
    "even his own kind, and for the first time offers a truce.",
  }, 3 },
  { "the matrix", {
    "a relic of leadership that will not open on command.",
    "it opens for the one who reaches without wanting to,",
    "and answers the devourer with blinding gold.",
  }, 3 },
  { "the decepticons", {
    "conquest was the old war. this one is different --",
    "the devourer consumes even them. some hold the line;",
    "one exile turns predator at the system's dark edge.",
  }, 3 },
  { "the spearcons", {
    "cryotek's lieutenants -- shardclaw and icerender.",
    "sticky spears that cling where they strike and count down;",
    "delayed detonation turns a debris field into a ticking clock.",
  }, 3 },
  { "forgeheart", {
    "an autobot who fights the terrain itself: flips a spinning",
    "asteroid to crush an ambush, raises debris into cover.",
    "heavy, deliberate, and hard to move once he sets his feet.",
  }, 3 },
  { "the solar engine", {
    "mercury relays. venus tempers air. earth holds the core.",
    "mars strips metal; the outer moons refine; pluto contains.",
    "the whole system was one machine. we just lived on its parts.",
  }, 3 },
  { "the reassembler", {
    "unicron was never built to destroy. he was built to rebuild --",
    "break dead worlds down, reforge them into living ones.",
    "a god-machine. the makers simply lost the controls.",
  }, 3 },
  { "sam witwicky", {
    "the human who has stood with the autobots longest.",
    "he builds the interface, keeps the link to old lore,",
    "and mentors the one who will carry it forward.",
  }, 3 },
  { "the asteroid graveyard", {
    "between mars and jupiter, a belt of broken war --",
    "cybertron's scattered fragments, dead outposts, drifting hulls.",
    "when the devourer woke, the whole belt woke with it.",
  }, 3 },
  { "the containment", {
    "they could not kill him, so they caged him instead --",
    "earth for the head, the rim for the spine, pluto for the field.",
    "the field is old now. as he stirs, the cage starts to give.",
  }, 3 },
};

int ex_lore_count(void){ return (int)(sizeof(ENTRIES)/sizeof(ENTRIES[0])); }
const ex_lore_entry_t* ex_lore_entry(int i){
  if (i<0||i>=ex_lore_count()) return NULL;
  return &ENTRIES[i];
}
