// SPDX-License-Identifier: LGPL-3.0-only
// Lookitan cursor relay for KWin (Plasma 6, Wayland).
// Adapted from MaruPop's marupopcursor script, (c) 2026 marunine, LGPL-3.0-only.
//
// Runs inside kwin_wayland's script engine, so it must never throw. KWin exports no pointer
// position over D-Bus and no Wayland protocol reports it to an unfocused client;
// workspace.cursorPos is the one source. The position is sent only when it changes, as strings
// so the D-Bus signature does not depend on how a number is marshalled. Lookitan's reply asks
// for the fast rate while a lookup is in progress.

var SERVICE = "io.github.arrowpoppa.Lookitan";
var OBJECT = "/Cursor";
var IFACE = "io.github.arrowpoppa.Lookitan.CursorSink";

var IDLE_MS = 100;
var ACTIVE_MS = 16;

var lastX = -1;
var lastY = -1;
var active = false;

var pump = new QTimer();
pump.interval = IDLE_MS;

function setActive(on) {
    if (on === active) {
        return;
    }
    active = on;
    pump.interval = on ? ACTIVE_MS : IDLE_MS;
}

pump.timeout.connect(function () {
    try {
        var p = workspace.cursorPos;
        if (p.x === lastX && p.y === lastY) {
            return;
        }
        lastX = p.x;
        lastY = p.y;
        var s = workspace.screenAt(p);
        callDBus(SERVICE, OBJECT, IFACE, "Update",
                 String(p.x), String(p.y),
                 s ? String(s.name) : "",
                 s ? String(s.devicePixelRatio) : "1",
                 String(Date.now()),
                 function (wantFast) { setActive(wantFast === true); });
    } catch (e) {
        // Never let an exception unload the script.
    }
});

pump.start();
