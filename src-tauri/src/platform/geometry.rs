// KWin reports logical desktop coordinates; XWayland/Windows report physical window coordinates.
// Keep both monitor rectangles so capture, borders, clicks and popup placement use the same map.

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct DisplayGeometry {
    pub desktop: Rect,
    pub native: Rect,
    pub scale: f64,
}

impl DisplayGeometry {
    pub fn to_native(self, x: f64, y: f64) -> (f64, f64) {
        (self.native.x + (x - self.desktop.x) * self.native.width / self.desktop.width,
         self.native.y + (y - self.desktop.y) * self.native.height / self.desktop.height)
    }

    pub fn to_desktop(self, x: f64, y: f64) -> (f64, f64) {
        (self.desktop.x + (x - self.native.x) * self.desktop.width / self.native.width,
         self.desktop.y + (y - self.native.y) * self.desktop.height / self.native.height)
    }

    pub fn css_rect(self, bounds: (f64, f64, f64, f64), origin: (f64, f64), scale: f64) -> (f64, f64, f64, f64) {
        let (x0, y0) = self.to_native(bounds.0, bounds.1);
        let (x1, y1) = self.to_native(bounds.2, bounds.3);
        ((x0 - origin.0) / scale, (y0 - origin.1) / scale, (x1 - x0) / scale, (y1 - y0) / scale)
    }

    pub fn css_to_desktop(self, x: f64, y: f64, origin: (f64, f64), scale: f64) -> (f64, f64) {
        self.to_desktop(origin.0 + x * scale, origin.1 + y * scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) { assert!((a - b).abs() < 0.001, "{a} != {b}"); }

    #[test]
    fn windows_physical_coordinates_keep_the_same_dpi_conversion() {
        let rect = Rect { x: -2560.0, y: 0.0, width: 2560.0, height: 1440.0 };
        let display = DisplayGeometry { desktop: rect, native: rect, scale: 2.0 };
        assert_eq!(display.css_rect((-2400.0, 100.0, -2200.0, 140.0), (-2560.0, 0.0), 2.0), (80.0, 50.0, 100.0, 20.0));
        assert_eq!(display.css_to_desktop(130.0, 60.0, (-2560.0, 0.0), 2.0), (-2300.0, 120.0));
    }

    #[test]
    fn kde_external_monitor_uses_xwayland_scale_and_actual_overlay_origin() {
        let display = DisplayGeometry {
            desktop: Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 },
            native: Rect { x: 0.0, y: 0.0, width: 2208.0, height: 1242.0 },
            scale: 1.0,
        };
        // The panel displaced the overlay 28 XWayland pixels below the requested origin.
        let (x, y, w, h) = display.css_rect((100.0, 100.0, 200.0, 120.0), (0.0, 28.0), 1.0);
        close(x, 115.0); close(y, 87.0); close(w, 115.0); close(h, 23.0);
        let (dx, dy) = display.css_to_desktop(x + w / 2.0, y + h / 2.0, (0.0, 28.0), 1.0);
        close(dx, 150.0); close(dy, 110.0);
    }

    #[test]
    fn kde_second_monitor_preserves_fractional_and_negative_origins() {
        for sign in [-1.0, 1.0] {
            let display = DisplayGeometry {
                desktop: Rect { x: sign * 1920.0, y: -100.0, width: 1670.0, height: 940.0 },
                native: Rect { x: sign * 2208.0, y: -115.0, width: 1920.0, height: 1080.0 },
                scale: 1.0,
            };
            let point = (display.desktop.x + 800.0, 300.0);
            let native = display.to_native(point.0, point.1);
            let restored = display.to_desktop(native.0, native.1);
            close(restored.0, point.0); close(restored.1, point.1);
            let (x, y, w, h) = display.css_rect((point.0, point.1, point.0 + 50.0, point.1 + 20.0),
                (display.native.x, display.native.y), 1.0);
            let clicked = display.css_to_desktop(x + w / 2.0, y + h / 2.0,
                (display.native.x, display.native.y), 1.0);
            close(clicked.0, point.0 + 25.0); close(clicked.1, point.1 + 10.0);
        }
    }
}
