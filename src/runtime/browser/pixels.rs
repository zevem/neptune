//! Worker-owned complete pixels; UI snapshots contain only accumulated damage.
use super::{Frame, protocol::Damage};
use eframe::egui;
use std::sync::Arc;

pub(super) struct Surface {
    image: Arc<egui::ColorImage>,
}
impl Surface {
    pub(super) fn new(size: [usize; 2]) -> Self {
        Self {
            image: Arc::new(egui::ColorImage::filled(size, egui::Color32::TRANSPARENT)),
        }
    }
    pub(super) fn size(&self) -> [usize; 2] {
        self.image.size
    }

    pub(super) fn update(&mut self, damage: Damage, bgra: &[u8]) {
        let size = self.image.size;
        if damage == Damage::full(size[0] as u32, size[1] as u32) {
            // Full-page animation can share its completed immutable pixels
            // with the UI, avoiding a second megapixel-scale snapshot copy.
            let pixels = bgra
                .chunks_exact(4)
                .map(|source| {
                    egui::Color32::from_rgba_premultiplied(
                        source[2], source[1], source[0], source[3],
                    )
                })
                .collect();
            self.image = Arc::new(egui::ColorImage::new(size, pixels));
            return;
        }
        // A previously published complete frame may still be uploading.
        // Preserve its owned pixels before applying incremental damage.
        let image = Arc::make_mut(&mut self.image);
        let stride = size[0];
        let width = damage.width as usize;
        for (row, bytes) in bgra.chunks_exact(width * 4).enumerate() {
            let start = (damage.y as usize + row) * stride + damage.x as usize;
            for (pixel, source) in image.pixels[start..start + width]
                .iter_mut()
                .zip(bytes.chunks_exact(4))
            {
                *pixel = egui::Color32::from_rgba_premultiplied(
                    source[2], source[1], source[0], source[3],
                );
            }
        }
    }

    pub(super) fn snapshot(
        &self,
        mut damage: Damage,
        previous: Option<&Frame>,
        position: [i32; 2],
    ) -> Frame {
        if let Some(previous) = previous.filter(|f| f.size == self.image.size) {
            damage = damage.union(previous.damage());
        }
        if damage == Damage::full(self.image.size[0] as u32, self.image.size[1] as u32) {
            return Frame {
                image: self.image.clone(),
                position,
                size: self.image.size,
                offset: [0, 0],
            };
        }
        let stride = self.image.size[0];
        let width = damage.width as usize;
        let mut pixels = Vec::with_capacity(width * damage.height as usize);
        for y in damage.y..damage.y + damage.height {
            let start = y as usize * stride + damage.x as usize;
            pixels.extend_from_slice(&self.image.pixels[start..start + width]);
        }
        Frame {
            image: Arc::new(egui::ColorImage::new(
                [width, damage.height as usize],
                pixels,
            )),
            position,
            size: self.image.size,
            offset: [damage.x as usize, damage.y as usize],
        }
    }
}

impl Frame {
    fn damage(&self) -> Damage {
        Damage {
            x: self.offset[0] as u32,
            y: self.offset[1] as u32,
            width: self.image.size[0] as u32,
            height: self.image.size[1] as u32,
        }
    }
    pub(super) fn texture(
        &self,
        ctx: &egui::Context,
        mut previous: Option<egui::TextureHandle>,
        name: String,
    ) -> Option<egui::TextureHandle> {
        if let Some(texture) = &mut previous
            && texture.size() == self.size
        {
            texture.set_partial(
                self.offset,
                self.image.clone(),
                egui::TextureOptions::LINEAR,
            );
            return previous;
        }
        // Initial, resized and newly shown surfaces arrive as complete images.
        // Never allocate a partial image as if it were the full page.
        if self.offset == [0, 0] && self.image.size == self.size {
            return Some(ctx.load_texture(name, self.image.clone(), egui::TextureOptions::LINEAR));
        }
        previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_frames_stay_immutable_while_worker_applies_new_damage() {
        let mut surface = Surface::new([3, 1]);
        surface.update(Damage::full(3, 1), &[10, 20, 30, 255].repeat(3));
        let uploading = surface.snapshot(Damage::full(3, 1), None, [0, 0]);
        let patch = Damage {
            x: 1,
            y: 0,
            width: 1,
            height: 1,
        };
        surface.update(patch, &[1, 2, 3, 255]);
        let updated = surface.snapshot(patch, Some(&uploading), [0, 0]);
        assert_eq!(uploading.image.pixels[1].to_array(), [30, 20, 10, 255]);
        assert_eq!(updated.image.pixels[1].to_array(), [3, 2, 1, 255]);
        surface.update(Damage::full(3, 1), &[4, 5, 6, 255].repeat(3));
        assert_eq!(updated.image.pixels[1].to_array(), [3, 2, 1, 255]);
    }
    #[test]
    fn pending_patches_keep_unchanged_pixels_in_the_gap() {
        let mut surface = Surface::new([5, 1]);
        surface.update(Damage::full(5, 1), &[10, 20, 30, 128].repeat(5));
        let left = Damage {
            x: 1,
            y: 0,
            width: 1,
            height: 1,
        };
        surface.update(left, &[1, 2, 3, 255]);
        let previous = surface.snapshot(left, None, [0, 0]);
        let right = Damage {
            x: 3,
            y: 0,
            width: 1,
            height: 1,
        };
        surface.update(right, &[4, 5, 6, 255]);
        let next = surface.snapshot(right, Some(&previous), [0, 0]);
        assert_eq!(next.size, [5, 1]);
        assert_eq!(next.offset, [1, 0]);
        assert_eq!(next.image.size, [3, 1]);
        assert_eq!(
            next.image
                .pixels
                .iter()
                .map(|p| p.to_array())
                .collect::<Vec<_>>(),
            [[3, 2, 1, 255], [30, 20, 10, 128], [6, 5, 4, 255]]
        );
    }
    #[test]
    fn patches_reuse_texture_and_resize_requires_complete_pixels() {
        let ctx = egui::Context::default();
        let mut surface = Surface::new([4, 4]);
        let full = surface.snapshot(Damage::full(4, 4), None, [0, 0]);
        let original = full.texture(&ctx, None, "preview".into()).unwrap();
        let id = original.id();
        let damage = Damage {
            x: 2,
            y: 1,
            width: 1,
            height: 1,
        };
        surface.update(damage, &[1, 2, 3, 255]);
        let patch = surface.snapshot(damage, None, [0, 0]);
        assert!(patch.texture(&ctx, None, "incomplete".into()).is_none());
        let texture = patch
            .texture(&ctx, Some(original), "preview".into())
            .unwrap();
        assert_eq!(texture.id(), id);
        assert_eq!(texture.size(), [4, 4]);
        let surface = Surface::new([2, 2]);
        let resized = surface.snapshot(Damage::full(2, 2), Some(&patch), [0, 0]);
        let texture = resized
            .texture(&ctx, Some(texture), "preview".into())
            .unwrap();
        assert_eq!(texture.size(), [2, 2]);
    }
}
