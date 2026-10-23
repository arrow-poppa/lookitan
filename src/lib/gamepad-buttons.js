// Names of the controller's buttons as the pad in hand prints them: settings store Xbox names
// (gamepad.rs), and a PlayStation pad shows ✕ ○ □ △, L1/R1/L2/R2 and Options/Create, as Harbor
// (arrow-poppa/streaming, MIT) and Lookupper's GamepadLayout do.

const SONY = /dualsense|dualshock|playstation|\bps[2-5]\b|vendor:\s*0*54c|\bsony\b/i;

/** "ps" for a PlayStation pad's name, else "xbox". */
export function detectLayout(name) {
    return SONY.test(String(name || '')) ? 'ps' : 'xbox';
}

const NAMES = {
    xbox: {LS: 'L3', RS: 'R3'},
    ps: {A: '✕', B: '○', X: '□', Y: '△', LB: 'L1', RB: 'R1', LT: 'L2', RT: 'R2', View: 'Create', Menu: 'Options', LS: 'L3', RS: 'R3'},
};
const DIRECTIONS = {Up: 'Direcional ↑', Down: 'Direcional ↓', Left: 'Direcional ←', Right: 'Direcional →'};

/** A button's label on the layout. */
export function buttonName(button, layout = 'xbox') {
    return DIRECTIONS[button] || (NAMES[layout] || NAMES.xbox)[button] || button;
}
