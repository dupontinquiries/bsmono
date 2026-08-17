/*
    animation.test.mjs — tests for contents/ui/code/animation.js

        node --test qml/tests/*.mjs

    Seven slide-in patterns whose only previous reviewer was the phone.
*/

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, "..", "contents", "ui", "code", "animation.js"), "utf8");
const Anim = new Function(`${source}; return { slideFrom, fract, noise };`)();

const COLS = 4;
const ALL_STYLES = [0, 1, 2, 3, 4, 5, 6];
const grid = (style, n = 12, speed = 1.0) =>
    Array.from({ length: n }, (_, i) => Anim.slideFrom(style, i, COLS, speed));

test("fract is always positive, which plain % is not", () => {
    assert.equal(Anim.fract(2.25), 0.25);
    assert.ok(Anim.fract(-2.25) > 0, "the bug this function exists to prevent");
    assert.equal(Math.round(Anim.fract(-2.25) * 100) / 100, 0.75);
});

test("every style produces finite values and no negative delays", () => {
    // A negative delay became a Timer interval of 1ms in QML — the tile
    // arrived instantly while its neighbours staggered, which reads as a
    // glitch rather than a pattern.
    for (const style of ALL_STYLES) {
        for (const v of grid(style, 24)) {
            for (const [k, n] of Object.entries(v)) {
                assert.ok(Number.isFinite(n), `${k} finite for style ${style}`);
            }
            assert.ok(v.delay >= 0, `style ${style} delay >= 0`);
            assert.ok(v.scale > 0, `style ${style} scale > 0`);
        }
    }
});

test("style 0 leaves a tile exactly where it belongs", () => {
    for (const v of grid(0)) {
        assert.deepEqual(
            { dx: v.dx, dy: v.dy, scale: v.scale, rotation: v.rotation, delay: v.delay },
            { dx: 0, dy: 0, scale: 1, rotation: 0, delay: 0 }
        );
    }
});

test("from below rises, and later rows come from further away", () => {
    const v = grid(1);
    assert.ok(v.every((x) => x.dy > 0 && x.dx === 0), "purely vertical");
    assert.ok(v[COLS * 2].dy > v[0].dy, "third row starts lower than the first");
    assert.ok(v[1].delay > v[0].delay, "staggered in reading order");
});

test("from edges pulls the two halves inward", () => {
    const v = grid(2);
    assert.ok(v[0].dx < 0, "leftmost column comes from the left");
    assert.ok(v[COLS - 1].dx > 0, "rightmost from the right");
    assert.ok(Math.abs(v[0].dx) > Math.abs(v[1].dx), "outer columns travel further");
});

test("cascade moves on both axes with a diagonal wavefront", () => {
    const v = grid(3);
    assert.ok(v[0].dx < 0 && v[0].dy > 0);
    // Tiles on the same diagonal (row + col equal) share a delay.
    assert.equal(v[1].delay, v[COLS].delay, "(0,1) and (1,0) arrive together");
    assert.ok(v[COLS + 1].delay > v[1].delay, "the wave advances");
});

test("ripple starts small and times by distance from the centre", () => {
    const v = grid(4);
    assert.ok(v.every((x) => x.scale === 0.3 && x.dx === 0 && x.dy === 0));
    const centre = Math.floor(COLS / 2) + COLS; // roughly the middle of a 3-row grid
    const corner = 0;
    assert.ok(v[centre].delay < v[corner].delay, "the centre goes first");
});

test("scatter is random-looking but identical on every open", () => {
    const a = grid(5);
    const b = grid(5);
    assert.deepEqual(a, b, "same index, same vector — no shimmer between opens");
    assert.ok(a.some((x) => x.dx > 0) && a.some((x) => x.dx < 0), "spread both ways");
    assert.ok(a.some((x) => x.rotation !== 0));
    assert.ok(a.every((x) => Math.abs(x.dx) <= 80 && Math.abs(x.dy) <= 80), "bounded");
    // Neighbours must not land on the same offset, or it stops being scatter.
    assert.notEqual(a[0].dx, a[1].dx);
    assert.notEqual(a[0].dy, a[1].dy);
});

test("wave staggers by column as a sine, symmetric about the middle", () => {
    const v = grid(6);
    assert.ok(v.every((x) => x.dy === 70));
    const first = v[0].delay;
    const last = v[COLS - 1].delay;
    assert.equal(first, last, "the two ends of a row start together");
    assert.ok(v[1].delay > first, "the middle of the row lags");
});

test("speed scales stagger and duration together", () => {
    const normal = Anim.slideFrom(1, 8, COLS, 1.0);
    const fast = Anim.slideFrom(1, 8, COLS, 0.6);
    assert.ok(fast.delay < normal.delay);
    assert.ok(fast.duration < normal.duration);
    assert.equal(fast.dy, normal.dy, "distance is a look, not a speed");
});

test("a single-column grid does not divide by zero", () => {
    // columns - 1 is the wave's divisor; a 1-column phone layout is legal.
    for (const style of ALL_STYLES) {
        const v = Anim.slideFrom(style, 3, 1, 1.0);
        assert.ok(Number.isFinite(v.delay) && v.delay >= 0, `style ${style}`);
    }
});
