// Moving between things with a controller's D-pad, in the pop-up (its controls) and the overlay
// (its words): the next one is the nearest whose centre lies that way, a step sideways counting
// twice a step forward, so moving right stays on the line.

/**
 * @param {{x: number, y: number}} from the current centre
 * @param {{x: number, y: number, w: number, h: number}[]} rects
 * @param {'up' | 'down' | 'left' | 'right'} dir
 * @returns {number} the index of the next rectangle, or -1
 */
export function nearest(from, rects, dir) {
    let best = -1;
    let bestScore = Infinity;
    rects.forEach((r, i) => {
        const dx = r.x + r.w / 2 - from.x;
        const dy = r.y + r.h / 2 - from.y;
        const [ahead, aside] = dir === 'left' ? [-dx, dy] : dir === 'right' ? [dx, dy] : dir === 'up' ? [-dy, dx] : [dy, dx];
        if (ahead <= 1) { return; }
        const score = ahead + 2 * Math.abs(aside);
        if (score < bestScore) { bestScore = score; best = i; }
    });
    return best;
}
