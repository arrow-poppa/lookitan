// SPDX-License-Identifier: LGPL-3.0-only
// Lookitan cursor relay for KWin (Plasma 6, Wayland).
// Adapted from MaruPop's marupopcursor script, (c) 2026 marunine, LGPL-3.0-only.
//
// Runs inside kwin_wayland's script engine, so it must never throw. KWin exports no pointer
// position over D-Bus and no Wayland protocol reports it to an unfocused client;
// workspace.cursorPos is the one source. Each report is one JSON string (callDBus takes at most
// nine arguments): pointer, the screen under it, and the active window, which Lookitan uses for
// per-app profiles. Lookitan's reply asks for the fast rate while a lookup is in progress.

var SERVICE = "io.github.arrowpoppa.Lookitan";
var OBJECT = "/Cursor";
var IFACE = "io.github.arrowpoppa.Lookitan.CursorSink";

var IDLE_MS = 100;
var ACTIVE_MS = 16;

var last = "";
var active = false;
var windowDirty = true;

var pump = new QTimer();
pump.interval = IDLE_MS;

function setActive(on) {
    if (on === active) {
        return;
    }
    active = on;
    pump.interval = on ? ACTIVE_MS : IDLE_MS;
}

function snapshot() {
    var p = workspace.cursorPos;
    var s = workspace.screenAt(p);
    var w = workspace.activeWindow;
    return {
        x: p.x,
        y: p.y,
        screen: s ? String(s.name) : "",
        scale: s ? s.devicePixelRatio : 1,
        sx: s ? s.geometry.x : 0,
        sy: s ? s.geometry.y : 0,
        sw: s ? s.geometry.width : 0,
        sh: s ? s.geometry.height : 0,
        appClass: w ? String(w.resourceClass) : "",
        appPid: w ? w.pid : 0,
        appTitle: w ? String(w.caption) : ""
    };
}

pump.timeout.connect(function () {
    try {
        var snap = snapshot();
        var key = snap.x + "," + snap.y + "," + snap.appPid + "," + snap.appTitle;
        if (key === last && !windowDirty) {
            return;
        }
        last = key;
        windowDirty = false;
        snap.sentAt = Date.now();
        callDBus(SERVICE, OBJECT, IFACE, "Update", JSON.stringify(snap),
                 function (wantFast) { setActive(wantFast === true); });
    } catch (e) {
        // Never let an exception unload the script.
    }
});

try {
    workspace.windowActivated.connect(function () { windowDirty = true; });
} catch (e) {
}

pump.start();
