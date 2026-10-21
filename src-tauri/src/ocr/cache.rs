// Small cache of detector boxes and recognized line groups. Pixels are fingerprinted,
// never retained here; changed frames and OCR settings always get a different key.
use std::collections::VecDeque;

use image::RgbaImage;
use ripemd::{Digest, Ripemd256};

use super::{paddle::TextBox, OcrLine};

pub type FrameKey = [u8; 32];
type BoxKey = Vec<[u32; 4]>;

#[derive(Default)]
pub struct OcrCache {
    frames: VecDeque<Frame>,
}

struct Frame {
    key: FrameKey,
    boxes: Option<Vec<TextBox>>,
    groups: VecDeque<(BoxKey, Vec<OcrLine>)>,
}

pub fn frame_key(image: &RgbaImage, context: &str) -> FrameKey {
    let mut hash = Ripemd256::new();
    hash.update(context.as_bytes());
    hash.update(image.width().to_le_bytes());
    hash.update(image.height().to_le_bytes());
    hash.update(image.as_raw());
    hash.finalize().into()
}

fn box_key(boxes: &[TextBox]) -> BoxKey {
    boxes
        .iter()
        .map(|b| {
            [
                b.x0.to_bits(),
                b.y0.to_bits(),
                b.x1.to_bits(),
                b.y1.to_bits(),
            ]
        })
        .collect()
}

impl OcrCache {
    fn frame(&mut self, key: FrameKey) -> &mut Frame {
        if let Some(index) = self.frames.iter().position(|frame| frame.key == key) {
            let frame = self.frames.remove(index).unwrap();
            self.frames.push_front(frame);
        } else {
            self.frames.push_front(Frame {
                key,
                boxes: None,
                groups: VecDeque::new(),
            });
            self.frames.truncate(3);
        }
        self.frames.front_mut().unwrap()
    }

    pub fn boxes(&mut self, key: FrameKey) -> Option<Vec<TextBox>> {
        self.frame(key).boxes.clone()
    }

    pub fn save_boxes(&mut self, key: FrameKey, boxes: &[TextBox]) {
        if boxes.len() <= 4096 {
            self.frame(key).boxes = Some(boxes.to_vec());
        }
    }

    pub fn lines(&mut self, key: FrameKey, boxes: &[TextBox]) -> Option<Vec<OcrLine>> {
        let frame = self.frame(key);
        let group = box_key(boxes);
        let index = frame
            .groups
            .iter()
            .position(|(stored, _)| *stored == group)?;
        let entry = frame.groups.remove(index).unwrap();
        let lines = entry.1.clone();
        frame.groups.push_front(entry);
        Some(lines)
    }

    pub fn save_lines(&mut self, key: FrameKey, boxes: &[TextBox], lines: &[OcrLine]) {
        // recognize_boxes suppresses individual failures; don't retain a partial/failed group.
        if lines.is_empty() || lines.len() != boxes.len() || lines.len() > 256 {
            return;
        }
        let frame = self.frame(key);
        let group = box_key(boxes);
        frame.groups.retain(|(stored, _)| *stored != group);
        frame.groups.push_front((group, lines.to_vec()));
        frame.groups.truncate(32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b() -> TextBox {
        TextBox {
            x0: 1.0,
            y0: 1.0,
            x1: 8.0,
            y1: 4.0,
        }
    }
    fn line() -> OcrLine {
        OcrLine {
            text: "word".into(),
            x0: 1.0,
            y0: 1.0,
            x1: 8.0,
            y1: 4.0,
            chars: vec![],
            ink_y: None,
            vertical: false,
        }
    }

    #[test]
    fn identical_pixels_reuse_detection_and_focus_but_changed_frame_or_language_miss() {
        let image = RgbaImage::new(10, 10);
        let key = frame_key(&image, "paddle|en-US|gpu");
        let mut cache = OcrCache::default();
        cache.save_boxes(key, &[b()]);
        cache.save_lines(key, &[b()], &[line()]);
        assert_eq!(cache.boxes(key).unwrap().len(), 1);
        assert_eq!(cache.lines(key, &[b()]).unwrap()[0].text, "word");
        let mut changed = image.clone();
        changed.put_pixel(5, 5, image::Rgba([1, 2, 3, 255]));
        assert!(cache
            .boxes(frame_key(&changed, "paddle|en-US|gpu"))
            .is_none());
        assert!(cache.boxes(frame_key(&image, "paddle|ja-JP|gpu")).is_none());
        assert!(cache.boxes(frame_key(&image, "paddle|en-US|cpu")).is_none());
    }

    #[test]
    fn focus_groups_are_independent_and_failed_recognition_is_not_cached() {
        let mut cache = OcrCache::default();
        let key = [0; 32];
        cache.save_lines(key, &[b()], &[]);
        assert!(cache.lines(key, &[b()]).is_none());
        cache.save_lines(key, &[b()], &[line()]);
        let other = TextBox { y0: 20.0, ..b() };
        assert!(cache.lines(key, &[other]).is_none());
        assert!(cache.lines(key, &[b()]).is_some());
    }

    #[test]
    fn frame_and_group_eviction_bounds_memory() {
        let mut cache = OcrCache::default();
        for i in 0..4 {
            cache.save_boxes([i; 32], &[b()]);
        }
        assert!(cache.boxes([0; 32]).is_none());
        for i in 0..33 {
            cache.save_lines(
                [9; 32],
                &[TextBox {
                    y0: i as f32,
                    ..b()
                }],
                &[line()],
            );
        }
        assert!(cache
            .lines([9; 32], &[TextBox { y0: 0.0, ..b() }])
            .is_none());
        assert!(cache
            .lines([9; 32], &[TextBox { y0: 32.0, ..b() }])
            .is_some());
    }
}
