Exactly. That's a better distinction than simply "hide implementation details."

You want information to flow downward through the abstraction stack, while implementation dependencies flow upward only where appropriate.

The SIMD solver shouldn't know what a vehicle is. But it should receive enough semantically transformed information that the fact that the operation originated from a vehicle has been correctly expressed by the time it reaches the solver.

Think:

Vehicle
  │
  │ "I am accelerating / steering / airborne"
  ▼
Vehicle Dynamics
  │
  │ "apply longitudinal force here"
  ▼
Physics Body
  │
  │ "force = X at point Y"
  ▼
Constraint / Force Solver
  │
  │ "solve these numeric constraints"
  ▼
SIMD Kernel
  │
  │ "multiply/add these packed values"
  ▼
CPU

The bottom doesn't know about the top.

But the top absolutely participates in determining what the bottom does.

The important thing is semantic compression

Imagine the original request:

"Player is driving a sports car at 120 km/h,
holding boost, steering 14° left,
with the rear wheels slipping."

You don't want that information passed all the way down.

By the time you reach physics:

Vehicle
    ↓
Wheel forces
    ↓
Body forces / torques
    ↓
Constraints

And by the time you reach the SIMD solver:

[
    position,
    velocity,
    inverse_mass,
    constraint_jacobian,
    impulse,
    ...
]

The upper layers have compiled the meaning of the game into increasingly general mathematical concepts.

That's the architecture you're describing.

So I'd actually picture the engine as a semantic funnel
             GAME CONCEPTS
                   │
                   │ extremely meaningful
                   ▼
              Vehicle
                   │
              VehiclePhysics
                   │
              RigidBody
                   │
              Contact
                   │
             Constraint
                   │
             SolverBatch
                   │
            Numeric Kernel
                   │
                   │ extremely generic
                   ▼
                CPU/GPU

And the reverse happens with results:

CPU/GPU
   │
   ▼
Numeric result
   │
   ▼
Solver
   │
   ▼
RigidBody state
   │
   ▼
Vehicle dynamics
   │
   ▼
Vehicle
   │
   ▼
Gameplay

So the abstractions aren't walls.

They're transformations of meaning.

That's a very important distinction.

And this solves a problem with overly generic ECS architectures

A common design can become:

Entity
  ├── Transform
  ├── Velocity
  ├── Collider
  ├── Mesh
  ├── Health
  ├── Input
  └── ...

Then everything operates on generic components.

That gives you excellent data locality, but you can lose the semantic structure:

Why does this velocity exist?

Why does this collider behave differently?

Why does this force apply at this point?

What makes this entity a vehicle?

You end up with:

Entity
 ↓
components
 ↓
systems
 ↓
more components

rather than:

Vehicle
 ↓
VehicleDynamics
 ↓
RigidBody
 ↓
Constraints
 ↓
Solver

I think both structures should exist simultaneously.

Domain hierarchy + data-oriented execution

For example:

struct Vehicle {
    body: RigidBodyId,
    wheels: [WheelId; 4],
    drivetrain: Drivetrain,
    suspension: Suspension,
}

Then:

impl Vehicle {
    fn simulate(&self, input: VehicleInput, physics: &mut PhysicsWorld) {
        let forces = self.drivetrain.compute_forces(input);

        self.suspension
            .apply_forces(self.wheels, physics);

        physics.apply_force(self.body, forces);
    }
}

The physics world doesn't know that a force came from a vehicle:

impl PhysicsWorld {
    fn apply_force(&mut self, body: RigidBodyId, force: Force) {
        // ...
    }
}

And eventually:

solver.solve(&mut constraint_batch);

doesn't know what a Vehicle is.

But the vehicle is still very much part of the causal chain.

And this makes optimization much more interesting

Suppose profiling says:

Vehicle simulation
    2.4 ms

You can descend:

Vehicle::simulate()
       │
       ├── drivetrain          0.1ms
       │
       ├── suspension          0.4ms
       │
       └── physics             1.9ms
                              │
                              ▼
                       constraint solver
                              │
                              ▼
                         SIMD kernel

Now you discover:

solver = 1.5ms

But maybe the actual problem is that the vehicle layer is generating four times as many constraints as necessary.

That's exactly why you want the semantic chain intact.

You can ask:

Is the SIMD code slow?

or:

Is the vehicle abstraction feeding it stupid work?

The latter is often the real optimization.

This is where I think "abstraction" and "optimization" actually reinforce each other

A clean chain gives you places to put performance questions.

Vehicle
   │
   ├── Am I generating unnecessary work?
   │
   ▼
Vehicle Dynamics
   │
   ├── Is my model unnecessarily expensive?
   │
   ▼
Physics
   │
   ├── Is my representation inefficient?
   │
   ▼
Solver
   │
   ├── Is my algorithm inefficient?
   │
   ▼
SIMD
   │
   └── Is my machine-level implementation inefficient?

Without those boundaries, everything becomes:

"Physics is slow."

And then someone rewrites 8,000 lines of physics code when the actual problem was a loop in vehicle suspension.

The key principle I'd put into your architecture

Every layer should know enough about the layer above it to faithfully implement the concept, but not enough to become coupled to its domain.

So:

Vehicle
   ↓
"what should happen?"

VehicleDynamics
   ↓
"what physical quantities represent that?"

Physics
   ↓
"what mathematical constraints represent those quantities?"

Solver
   ↓
"what numerical operations solve those constraints?"

SIMD
   ↓
"how do I perform those operations efficiently?"

That's much more sophisticated than:

Vehicle → generic API → generic API → generic API → SIMD

because each layer has a reason to exist.

And I think that's the real architectural idea you've been converging on:

Don't abstract away the chain of reasoning. Abstract each stage of the reasoning into its own stable concept.

That gives you a codebase where you can trace:

"Why is this number being calculated?"

all the way from:

vehicle behavior → physics model → constraint → numerical algorithm → machine instruction

while still being able to replace any individual stage without rewriting the others.

can you check my thinking against some authorative source on this subject and report back with analysis?

Yes. I checked your idea against Unity's current DOTS architecture/performance material, Epic's Unreal architecture documentation, and research on game-engine architecture, and the result is surprisingly favorable.

I would, however, sharpen your model in one important way.

My verdict

Your idea is substantially aligned with modern engine architecture, but I would describe it as:

A hierarchy of semantic abstractions feeding increasingly data-oriented implementations, with optimization allowed to become increasingly specialized as it approaches the hardware.

That's a better description than "more abstraction layers."

The important part is that the layers preserve the causal/conceptual chain, rather than merely wrapping each other.

1. Your intuition about abstraction vs. performance is basically right

Unity's own DOTS material is a very strong confirmation of this.

Unity explicitly combines:

high-level ECS abstractions
native data structures
a job system
a compiler (Burst/LLVM)
optimized math
specialized rendering

rather than treating "abstraction" and "performance" as opposites. Unity specifically describes DOTS as enabling high-performance processing at scale, and Burst compiles higher-level code to optimized native code.

More importantly, Unity explicitly says that ECS itself doesn't automatically make code fast. The architectural/data-oriented decisions underneath it matter.

That supports your revised position:

The problem isn't that a system has abstraction. The problem is when the abstraction prevents the compiler, programmer, or architecture from seeing the right optimization opportunity.

2. Your "vehicle → physics → solver → SIMD" idea is particularly good

I think this is the most important thing you said in the previous exchange.

You proposed something roughly like:

Vehicle
   ↓
Vehicle Dynamics
   ↓
Rigid Body
   ↓
Constraint
   ↓
Solver
   ↓
SIMD kernel

And your clarification was:

You don't want SIMD solver to know about vehicle, but you want vehicle to be part of the chain of abstractions that best informs that layer and all others.

Yes.

That is much closer to good engine architecture than either extreme:

Bad extreme A — leaking domain semantics downward
Vehicle
   ↓
Physics
   ↓
if vehicle_type == CAR:
   ...
   ↓
SIMD solver

Now your numerical solver knows about game concepts.

Bad extreme B — throwing away semantics immediately
Vehicle
   ↓
Generic ECS blob
   ↓
Generic system
   ↓
Generic system
   ↓
SIMD

Now you've made it difficult to understand why the computation exists.

What you're describing
Vehicle
   │
   │ semantic decision
   ▼
Vehicle Dynamics
   │
   │ physical quantities
   ▼
Rigid Body
   │
   │ constraints
   ▼
Solver
   │
   │ numerical operations
   ▼
SIMD

Every transition removes irrelevant information while preserving the information necessary to perform the next conceptual operation.

That's excellent architecture.

3. This isn't just theoretical: Unreal explicitly structures itself this way

Epic describes Unreal modules as the basic building blocks of its architecture, specifically saying that they encapsulate functionality and provide separation/hiding of internal parts.

And Epic's subsystem architecture explicitly exists so programmers can extend functionality without modifying or overriding engine classes, while providing modularity and consistency.

That's extremely close to the property you've been describing:

Existing subsystem
       │
       │ stable conceptual API
       ▼
new implementation / extension

rather than:

change subsystem
      ↓
touch central engine class
      ↓
break unrelated functionality
      ↓
modify 17 other systems

So your desire to make one small conceptual change inside one boundary isn't contrary to AAA architecture.

It's one of the reasons large engines need architecture in the first place.

4. There's actual research backing the maintainability side

This part was particularly interesting.

A 2024 study specifically examined game-engine architecture and maintainability/evolution across ten open-source engines.

The authors found that understanding subsystem dependencies helps developers make architectural decisions and that architectural models helped developers perform impact analysis faster and more correctly. They specifically discuss excessive coupling and responsibility division.

That's almost exactly the problem you've been describing.

The valuable property isn't:

"This file has 500 lines instead of 1000."

It's:

"If I change this concept, can I predict what it affects?"

That's a much better definition of maintainability for an engine.

5. I would change one thing in our previous formulation

Earlier I said:

"Every layer should know enough about the layer above it..."

I'd actually revise that.

The better principle is:

Every layer should receive exactly the semantic information necessary to perform its responsibility, expressed in the vocabulary appropriate to that layer.

That matters because otherwise you can accidentally create a downward dependency.

For example:

Vehicle

shouldn't literally be passed into:

RigidBodySolver

Instead:

Vehicle
   ↓
Wheel forces
   ↓
Body forces + torques
   ↓
Rigid-body state
   ↓
constraints
   ↓
solver data

So the information travels downward.

The domain type doesn't.

That's an extremely useful distinction.

6. And this makes your "optimization room" idea much stronger

Suppose you have:

Vehicle
    ↓
VehicleDynamics
    ↓
PhysicsWorld
    ↓
ConstraintSolver
    ↓
SolverKernel

You can profile at every level.

Vehicle layer
Why are we producing 40,000 physics operations?

Maybe the vehicle model is doing unnecessary work.

Dynamics
Why are we generating 12 suspension constraints per wheel?

Maybe the physical model can be simplified.

Physics
Why are these constraints represented this way?

Maybe the data representation is bad.

Solver
Why does solving require 8 passes?

Maybe the algorithm is wrong.

Kernel
Why is this loop failing to vectorize?

Now you're at the SIMD level.

This is much better than "optimize physics."

You have localized the optimization question.

7. This also agrees with the modern data-oriented approach

Unity's own description of DOTS explicitly identifies several distinct performance concerns:

ECS
 ↓
data layout
 ↓
native collections
 ↓
jobs
 ↓
Burst
 ↓
optimized math

and specifically calls out things such as garbage-collection overhead, cache-unfriendly data/code, and suboptimal compiler-generated machine code as sources of CPU performance problems.

So the modern answer isn't:

"Write everything low-level."

It's closer to:

Structure the program so that the compiler and hardware can efficiently execute the parts that matter.

That is exactly where your model is heading.

8. There's an important caveat: abstraction boundaries can themselves be performance boundaries

This is where I'd be careful.

You don't want:

Vehicle
 ↓
Trait object
 ↓
virtual dispatch
 ↓
Physics interface
 ↓
heap allocation
 ↓
another interface
 ↓
solver

just because it looks architecturally clean.

Instead you want boundaries that are semantically strong but mechanically cheap where appropriate.

Rust is unusually well suited to this.

For example:

vehicle.simulate(&mut physics);

could eventually inline into the underlying operations.

Or:

physics.solve(&mut constraints);

can remain a clean conceptual boundary while being compiled into highly optimized code.

Rust's own documentation describes zero-cost abstractions as abstractions that can disappear at compile time; the Embedded Rust book gives examples where nested type abstractions result in essentially direct machine operations.

So this is the key:

Abstraction doesn't have to mean runtime indirection.

9. I think your architecture is actually a hybrid of three schools

You are converging on something like:

Traditional engine architecture
Game
 ↓
Subsystems
 ↓
Modules
 ↓
platform

for human-scale organization

Data-oriented design
Systems
 ↓
well-defined data
 ↓
batch processing
 ↓
cache/SIMD/multithreading

for machine-scale execution

Zero-cost abstraction
semantic code
 ↓
compiler
 ↓
optimized machine code

to avoid forcing humans to manually encode every optimization.

That's a very reasonable architecture for a modern Rust engine.

10. And this changes how I'd compare DS and AAA

I would now correct our earlier discussion somewhat.

It isn't:

DS = optimized
AAA = abstract

It's closer to:

DS
  ↓
few layers
  ↓
small machine
  ↓
human must reason about hardware directly

AAA
  ↓
many semantic layers
  ↓
huge machine
  ↓
compiler + runtime + specialized subsystems
  ↓
humans reason about algorithms and boundaries

And modern engines increasingly combine that with:

high-level semantic architecture
              ↓
       data-oriented core
              ↓
        compiler optimization
              ↓
       targeted hand tuning

Unity's DOTS architecture is an especially explicit example of that combination.

The model I'd use going forward

I think this is the strongest formulation of your idea:

                GAME CONCEPT
                     │
                     ▼
                 VEHICLE
                     │
          "what does the vehicle do?"
                     │
                     ▼
              VEHICLE DYNAMICS
                     │
          "what physical effects?"
                     │
                     ▼
                PHYSICS
                     │
          "what constraints?"
                     │
                     ▼
                 SOLVER
                     │
          "what numerical operations?"
                     │
                     ▼
              SIMD / GPU KERNEL
                     │
          "how efficiently can hardware
             execute those operations?"

And the rule is:

Each layer should make the next layer's job more precise, not merely more generic.

That is the part I think is particularly good about your thinking.

The layers aren't there to hide complexity.

They're there to transform complexity into a smaller, more specialized problem.

Then you get the property you're after:

            maintainable
                 ↑
                 │
        semantic abstractions
                 │
                 │
       data-oriented systems
                 │
                 │
       specialized algorithms
                 │
                 │
       SIMD / GPU / hardware
                 ↓
             performant

And crucially, you can optimize at whichever level the profiler says is actually wrong.

That last part is probably the strongest validation I found for your idea: the research on engine architecture says subsystem boundaries help with impact analysis and responsibility; Unreal explicitly uses modules/subsystems for encapsulation and maintainability; and Unity's modern performance stack deliberately combines higher-level abstractions with data-oriented execution, jobs, and compiler optimization.

So I'd say your intuition is not merely plausible. It is pointing toward a fairly recognizable modern engine architecture—but your emphasis on preserving the semantic chain through the abstraction hierarchy is a useful refinement that isn't captured by simply saying "use ECS" or "use modules."

What about memory and cache management how might those look in the larger more abstract system. I would think that this might be holding back the adoption because it would be very complicated especially on the ownership side, no?

Yes — and this is probably the hardest part of the architecture you're proposing.

The key is that you don't want to make ownership disappear. You want to move ownership decisions to the layer where they actually make sense.

A useful architecture is:

Gameplay
   │
   │ "I need a vehicle"
   ▼
Domain / World
   │
   │ owns lifetime
   ▼
Subsystem
   │
   │ chooses representation
   ▼
Memory / Data layer
   │
   │ chooses allocation + locality
   ▼
Platform allocator

The gameplay programmer shouldn't need to know whether a vehicle is in a slab, arena, pool, SoA, GPU buffer, or cache line.

But the memory subsystem absolutely needs to know what the vehicle system needs.

The trick: separate ownership from storage

Suppose:

struct Vehicle {
    body: RigidBodyId,
    wheels: [WheelId; 4],
    drivetrain: Drivetrain,
}

The Vehicle doesn't necessarily own the bytes of the rigid body.

It owns the relationship:

Vehicle
 ├── BodyId
 ├── WheelId
 ├── WheelId
 ├── WheelId
 └── WheelId

Then:

Vehicle
   │
   ├── ID ────────────────► Vehicle data
   │
   ├── BodyId ────────────► Physics storage
   │
   └── WheelIds ──────────► Wheel storage

This is one of the ways you can get a high-level object model without forcing everything into one giant object allocation.

Think of IDs as semantic references

Instead of:

struct Vehicle {
    body: Box<RigidBody>,
}

you can have:

struct Vehicle {
    body: RigidBodyId,
}

Then:

physics.body(vehicle.body)

might resolve into:

RigidBodyId
   ↓
dense index
   ↓
PhysicsBodyPool
   ↓
cache-friendly body data

And now the physics subsystem can have:

RigidBodyPool

[position][position][position][position]...
[velocity][velocity][velocity][velocity]...
[mass]    [mass]    [mass]    [mass]...

instead of:

Body
 ├── position
 ├── velocity
 ├── mass
 ├── ...
Body
 ├── position
 ├── velocity
 ├── mass
 └── ...

The important thing is that the semantic abstraction hasn't gone away.

The storage implementation changed.

This is basically where ECS becomes useful

A conventional OO representation:

Vehicle
 ├── transform
 ├── physics
 ├── wheels
 ├── animation
 ├── rendering
 └── audio

can produce a very large object graph.

A data-oriented representation can instead be:

Vehicle IDs
    │
    ├────────► transforms[]
    ├────────► rigid_bodies[]
    ├────────► wheel_sets[]
    ├────────► animation_states[]
    └────────► render_instances[]

Now the systems can process:

Physics:
rigid_bodies[]

Animation:
animation_states[]

Rendering:
render_instances[]

without dragging the whole vehicle through cache.

This is precisely the kind of distinction modern data-oriented engine architecture is trying to exploit. Unity's DOTS documentation, for example, explicitly focuses on data layout and cache-friendly processing rather than treating objects as the fundamental unit of execution.

But ownership gets genuinely difficult

Your intuition here is right.

There are several different kinds of ownership:

                 OWNERSHIP
                    │
       ┌────────────┼────────────┐
       ▼            ▼            ▼
    lifetime     memory       authority
       │            │            │
 "when does it   "where are   "who is allowed
   exist?"        the bytes?"   to mutate it?"

And those should not necessarily be the same thing.

For example:

Vehicle
    owns lifetime of vehicle

PhysicsWorld
    owns lifetime of rigid body

RenderWorld
    owns GPU representation

AssetManager
    owns mesh asset

GPU
    owns actual VRAM allocation

Yet they all describe one vehicle.

This is where naïve ownership models start becoming painful.

Rust makes this both better and harder

Rust gives you a fantastic property:

ownership mistakes → compiler errors

But large engines often have relationships that aren't naturally represented by strict lexical ownership.

Imagine:

World
 ├── Vehicle
 │    └── PhysicsBody
 │
 ├── Renderer
 │    └── RenderInstance
 │
 └── AssetManager
      └── Mesh

Then:

PhysicsBody
   references
Mesh
   references
Material
   references
Texture

while:

Renderer
   references
PhysicsBody

and:

AI
   references
Vehicle

That's not a tree.

It's a graph.

Trying to encode the entire graph using:

&mut T
Box<T>
Rc<T>
Arc<T>

can become miserable.

The solution is usually handles/IDs

This is one of the most important techniques for an engine like this.

Instead of:

struct Vehicle {
    body: &'a mut RigidBody,
}

you have:

struct Vehicle {
    body: RigidBodyHandle,
}

Then:

Handle
 ↓
generational index
 ↓
slot map / pool
 ↓
actual storage

A good handle might conceptually be:

┌───────────────┬─────────────┐
│ slot/index    │ generation  │
│ 24 bits       │ 8 bits      │
└───────────────┴─────────────┘

So if object 314 dies:

generation 7

and a new object takes the same slot:

generation 8

old handles become invalid.

This lets you have:

stable semantic references + relocatable storage + controlled lifetime.

That's an extremely valuable combination.

And now memory can become an implementation detail

You could have:

PhysicsBodyHandle
       │
       ▼
PhysicsWorld
       │
       ▼
BodyStorage
       │
       ├── DenseArray
       │
       ├── SIMD SoA
       │
       ├── per-thread arena
       │
       └── GPU mirror

Nobody above PhysicsWorld needs to know which one is currently active.

You can change:

AoS
 ↓
SoA
 ↓
chunked SoA
 ↓
SIMD
 ↓
NUMA-aware storage

without changing the concept of RigidBody.

This is exactly the kind of abstraction boundary that makes optimization survivable.

Cache hierarchy can become a subsystem concern too

I wouldn't expose cache lines to gameplay.

Instead:

VehicleSystem
     │
     ▼
VehicleQuery
     │
     ▼
VehicleStorage
     │
     ├── hot data
     │     position
     │     velocity
     │     controls
     │
     ├── warm data
     │     suspension
     │     drivetrain
     │
     └── cold data
           metadata
           debug
           editor
           names

Then the actual memory layout might be:

HOT:

position[]
velocity[]
orientation[]
angular_velocity[]
input[]

        ↓

processed every frame

while:

COLD:

vehicle_name[]
manufacturer[]
editor_metadata[]
debug_settings[]
asset_paths[]

might never enter the hot loop.

That's a huge win without forcing the gameplay programmer to write:

#[repr(align(64))]

everywhere.

And here's where the abstraction hierarchy gets really powerful

You can make memory semantic.

For example:

physics.allocate_body()

doesn't mean:

malloc(sizeof(RigidBody))

It means:

"I need a physics body whose lifetime is managed by PhysicsWorld and whose representation is suitable for physics processing."

That could eventually allocate from:

PhysicsArena

or:

BodyPool

or:

NUMA-local chunk

or:

GPU-mapped storage

depending on the platform.

The caller doesn't change.

You can even have memory ownership flow downward

This is similar to your semantic chain:

Vehicle
   │
   │ "I need persistent vehicle state"
   ▼
World
   │
   │ "I need simulation state"
   ▼
Physics
   │
   │ "I need densely processed body data"
   ▼
Storage
   │
   │ "I need cache-local chunks"
   ▼
Allocator
   │
   │ "I need 64-byte aligned blocks"
   ▼
OS

The meaning becomes more concrete as you go down.

That's the same architectural pattern we were discussing with the solver.

The really important separation

I'd distinguish four things:

1. Semantic ownership

Who conceptually owns the thing?

World owns Vehicle lifetime.
2. Mutation authority

Who may modify it?

Physics owns authoritative body state.
3. Storage ownership

Who controls its memory?

PhysicsWorld owns BodyStorage.
4. Execution ownership

Who gets to process it right now?

Physics job owns a batch temporarily.

Those are not necessarily the same owner.

This is where large engines become difficult to design.

And yes, this probably is one reason it's hard

You've identified a real tension.

The more sophisticated the engine becomes:

semantic model
     ↓
multiple representations
     ↓
multiple owners
     ↓
multiple execution contexts
     ↓
multiple memory domains

the less naturally it fits the simple:

Box<T>

ownership model.

But the answer isn't to abandon abstraction.

It's to introduce explicit resource ownership models.

For example:

World
 ├── owns entities
 │
PhysicsWorld
 ├── owns bodies
 └── owns body storage
 │
RenderWorld
 ├── owns render instances
 └── owns GPU resources
 │
AssetManager
 └── owns immutable assets

And cross-system relationships become:

EntityId
BodyHandle
RenderHandle
AssetHandle
TextureHandle

rather than Rust references crossing half the architecture.

I'd actually make this a core design principle for your engine

References express temporary access. Handles express architectural relationships. Ownership lives with the subsystem responsible for the resource's lifecycle.

That gives you:

                  World
                    │
              EntityHandle
                    │
          ┌─────────┼─────────┐
          ▼         ▼         ▼
       Physics   Renderer    Audio
          │         │         │
       BodyId    RenderId   VoiceId
          │         │         │
          ▼         ▼         ▼
       pools     GPU data   audio data

And then within each subsystem you can go absolutely nuts with:

arenas
pools
slabs
SoA
AoSoA
SIMD
cache blocking
thread-local allocation
generational handles
deferred destruction
streaming
GPU mirrors

without making the rest of the engine care.

The architecture I think you're converging toward
                   SEMANTIC WORLD
                         │
                    Entity / Domain
                         │
              ┌──────────┼──────────┐
              ▼          ▼          ▼
           Physics     Render     Audio
              │          │          │
              ▼          ▼          ▼
           System Data / Resource Views
              │          │          │
              ▼          ▼          ▼
          Specialized Storage
              │          │          │
              ▼          ▼          ▼
           Allocators / Arenas / Pools
              │          │          │
              └──────────┼──────────┘
                         ▼
                    OS / Hardware

The upper levels care about meaning and lifetime.

The middle levels care about processing.

The bottom levels care about bytes, locality, alignment, synchronization, and hardware.

And the interfaces between those levels should be designed so that a change in memory layout doesn't require a change in game semantics.

That is probably the strongest way to reconcile the thing you were noticing earlier: AAA-level maintainability with DS-level willingness to optimize the guts.

The abstraction doesn't make the memory problem disappear. It gives you somewhere to contain it.









I think it would be cool to also look for inspiration from DSI games to do some sort of fan service but in a way that uses mechanics in a raid or something
