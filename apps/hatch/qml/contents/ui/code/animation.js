/*
    animation.js — where a tile starts before it slides into place.

    Extracted from ItemGridDelegate so it can be executed off the phone. Each
    pattern is a pure function of the tile's grid position, returning the
    vector and stagger the delegate animates away from; nothing here touches
    QML types, so `node --test qml/tests/` covers all seven patterns. Getting
    this wrong is invisible in review and obvious on screen — tiles arriving
    from nowhere, or arriving late enough to look like a stutter.

    Styles (index-compatible with the tileSlideStyle config key):
      0 none  1 below  2 edges  3 cascade  4 ripple  5 scatter  6 wave
*/

/** Fractional part, always in [0, 1). JS `%` keeps the sign of the operand,
    so `x % 1` on a negative x is negative — which turned into negative
    stagger delays the first time this was written inline. */
function fract(x) {
    return x - Math.floor(x);
}

/** Deterministic per-index pseudo-random in [0, 1): the same tile scatters to
    the same place every open, so repeated launches don't shimmer. */
function noise(index, salt) {
    return fract(Math.sin((index + 1) * 127.1 + salt * 311.7) * 43758.5453);
}

/**
 * The starting offset for a tile.
 *
 * @param {int} style      tileSlideStyle
 * @param {int} index      linear index in the grid
 * @param {int} cols       columns currently laid out
 * @param {real} speed     animation speed multiplier (1.6 slow … 0.6 fast)
 * @return {object} { dx, dy, delay, scale, rotation, duration }
 */
function slideFrom(style, index, cols, speed) {
    const columns = Math.max(1, cols);
    const col = index % columns;
    const row = Math.floor(index / columns);
    const centerCol = Math.floor(columns / 2);

    let dx = 0, dy = 0, delay = 0, scale = 1.0, rotation = 0;

    switch (style) {
    case 1: // from below, later rows travelling further
        dy = 60 + (row * 12);
        delay = index * 18;
        break;
    case 2: // from the left and right edges, outside-in
        dx = (col < centerCol ? -1 : 1) * (Math.abs(col - centerCol) + 1) * 30;
        delay = (columns - Math.abs(col - centerCol)) * 22;
        break;
    case 3: // diagonal wavefront
        dx = -40;
        dy = 40;
        delay = (row + col) * 26;
        break;
    case 4: { // ripple outward from the grid's centre
        const dr = row - 1.5;
        const dc = col - centerCol;
        scale = 0.3;
        delay = Math.sqrt(dr * dr + dc * dc) * 40;
        break;
    }
    case 5: { // scatter, deterministic per tile
        const a = noise(index, 0);
        const b = noise(index, 1);
        dx = (a - 0.5) * 160;
        dy = (b - 0.5) * 160;
        rotation = (a - 0.5) * 40;
        scale = 0.5;
        delay = a * 180;
        break;
    }
    case 6: // up from below, sinusoidal by column
        dy = 70;
        delay = Math.sin(col / Math.max(1, columns - 1) * Math.PI) * 90 + row * 30;
        break;
    default: // 0 and anything unknown: already in place
        break;
    }

    return {
        dx: dx,
        dy: dy,
        // Speed scales stagger and duration together, so "fast" stays a
        // coherent motion rather than a quick move with a long wait.
        delay: Math.max(0, Math.round(delay * speed)),
        scale: scale,
        rotation: rotation,
        duration: Math.round(300 * speed),
    };
}
