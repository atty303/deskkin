use crate::blend_rgb565;

/// Source colors are native RGB565; destination words use the requested order.
/// Slices have equal lengths. Implementations must not access outside them.
pub trait Blitter {
    fn blit(&mut self, destination: &mut [u16], source: &[u16], alpha: Option<&[u8]>, wire: bool);

    /// Draw `destination.len()` pixels starting at `offset`. The entire source
    /// and optional alpha backing slices are readable, including padding;
    /// only the destination slice is writable. The selected range must exist.
    fn blit_from(
        &mut self,
        destination: &mut [u16],
        source: &[u16],
        offset: usize,
        alpha: Option<&[u8]>,
        wire: bool,
    ) {
        let end = offset.checked_add(destination.len()).expect("blit range");
        self.blit(
            destination,
            &source[offset..end],
            alpha.map(|a| &a[offset..end]),
            wire,
        );
    }
}

pub struct ScalarBlitter;

impl Blitter for ScalarBlitter {
    fn blit(&mut self, destination: &mut [u16], source: &[u16], alpha: Option<&[u8]>, wire: bool) {
        assert_eq!(destination.len(), source.len());
        assert!(alpha.is_none_or(|a| a.len() == source.len()));
        for (index, (dst, &src)) in destination.iter_mut().zip(source).enumerate() {
            let a = alpha.map_or(255, |a| a[index]);
            if a == 0 {
                continue;
            }
            let color = if a == 255 {
                src
            } else {
                blend_rgb565(if wire { u16::from_be(*dst) } else { *dst }, src, a)
            };
            *dst = if wire { color.to_be() } else { color };
        }
    }
}
