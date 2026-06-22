# Graph card taps dead on touch (dogfood v0.1.734 → fix v0.1.735)

## Symptom
On the phone (postmarketOS), tapping a graph card registered **nothing** — no
preview, no focus. Localized by the user: graph cards only; typing, terminal,
and other UI fine. Worked on desktop with a mouse.

## Diagnosis (no GUI in sandbox — reasoned from Qt semantics)
The cards canvas has a `DragHandler` (`bgPanDrag`, target:null) that drives the
Flickable pan. PointerHandlers use Qt's exclusive-grab system. With no
`grabPermissions` set, the DragHandler grabs the touch point on press and does
not relinquish it, so a tap landing on a card is consumed by the pan handler and
the card's `MouseArea` (`cardDrag`) never receives its `onReleased` → the
`!_didDrag` tap branch (open preview) never runs.

Why it worked with a mouse: a mouse press+release at the same point never
crosses the DragHandler's drag threshold, so the handler stays inactive and the
MouseArea click fires normally. Touch is more eager to grab, exposing the
conflict. `preventStealing: true` on the MouseArea does NOT help — it governs
the old Item/Flickable filtering path, not PointerHandler exclusive grabs.

## Fix
Set `grabPermissions` on `bgPanDrag` so an Item (the card MouseArea) may take
over the grab for a stationary tap:

    grabPermissions: PointerHandler.CanTakeOverFromHandlersOfDifferentType
        | PointerHandler.CanTakeOverFromItems
        | PointerHandler.ApprovesTakeOverByItems
        | PointerHandler.ApprovesTakeOverByHandlersOfDifferentType

`ApprovesTakeOverByItems` is the key flag: it lets the card MouseArea steal the
grab for a tap. A real drag on empty background still pans (the handler engages
once the pointer moves past threshold on a non-card press).

## Confidence / verification
RUNTIME-UNTESTED — touch grabs aren't sandbox-reproducible (no GUI/Vulkan). This
is the documented mechanism for MouseArea-vs-PointerHandler grab conflicts, and
qml_lint is clean, but it needs on-device confirmation.

Risk: if the card MouseArea now grabs too eagerly, background PAN starting on top
of a card could suffer (you'd have to start the pan on empty space). If that
regresses, the rollback is to remove the `grabPermissions` line; the deeper fix
would be a TapHandler on the card (which composes with the pan handler via the
grab system) instead of relying on the MouseArea release branch.
