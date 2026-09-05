use crate::{RasterError, SourceSize, Texture, TextureRegion, validate_texture};

/// Reduces one atlas region by two, averaging premultiplied color and coverage.
/// Odd edges repeat their last texel. Output is straight RGB565 plus A8, with
/// transparent padding. Source and output never alias and no allocation occurs.
pub fn downsample(
    texture: Texture<'_>,
    region: TextureRegion,
    colors: &mut [u16],
    alpha: &mut [u8],
    stride: u16,
) -> Result<SourceSize, RasterError> {
    validate_texture(texture, region)?;
    let size = SourceSize {
        width: region.width.div_ceil(2),
        height: region.height.div_ceil(2),
    };
    let required = usize::from(stride) * usize::from(size.height);
    if stride < size.width || colors.len() != required || alpha.len() != required {
        return Err(RasterError::InvalidTexture);
    }
    colors.fill(0);
    alpha.fill(0);
    for y in 0..size.height {
        let y0 = y * 2;
        let y1 = (y0 + 1).min(region.height - 1);
        for x in 0..size.width {
            let x0 = x * 2;
            let x1 = (x0 + 1).min(region.width - 1);
            let indices = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)].map(|(x, y)| {
                usize::from(region.source_y + y) * usize::from(region.stride)
                    + usize::from(region.source_x + x)
            });
            let coverage = indices.map(|i| u32::from(texture.coverage.at(i)));
            let sum: u32 = coverage.iter().sum();
            let out = usize::from(y) * usize::from(stride) + usize::from(x);
            alpha[out] = ((sum + 2) / 4) as u8;
            if sum == 0 {
                continue;
            }
            for (shift, mask) in [(0, 31u16), (5, 63), (11, 31)] {
                let weighted: u32 = indices
                    .iter()
                    .zip(coverage)
                    .map(|(&i, a)| u32::from((texture.pixels[i] >> shift) & mask) * a)
                    .sum();
                colors[out] |= (((weighted + sum / 2) / sum) as u16) << shift;
            }
        }
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Coverage, Mask8, build_opaque_mask};

    #[test]
    fn transparent_color_cannot_bleed_and_padding_stays_transparent() {
        let size = SourceSize {
            width: 3,
            height: 3,
        };
        let pixels = [
            0xf800, 0x001f, 0x001f, 0x001f, 0x001f, 0x001f, 0x001f, 0x001f, 0x07e0,
        ];
        let a = [255, 0, 0, 0, 0, 0, 0, 0, 255];
        let mut mask = [0];
        build_opaque_mask(size, &a, &mut mask).unwrap();
        let texture = Texture {
            size,
            pixels: &pixels,
            coverage: Coverage::Alpha8 {
                alpha: &a,
                opaque_blocks: Mask8::new(size, &mask).unwrap(),
            },
        };
        let mut colors = [0xffff; 8];
        let mut alpha = [255; 8];
        let region = TextureRegion {
            source_x: 0,
            source_y: 0,
            width: 3,
            height: 3,
            stride: 3,
        };
        assert_eq!(
            downsample(texture, region, &mut colors, &mut alpha, 4),
            Ok(SourceSize {
                width: 2,
                height: 2
            })
        );
        assert_eq!(colors, [0xf800, 0, 0, 0, 0, 0x07e0, 0, 0]);
        assert_eq!(alpha, [64, 0, 0, 0, 0, 255, 0, 0]);
        let before = colors;
        assert!(downsample(texture, region, &mut colors, &mut alpha, 1).is_err());
        assert_eq!(colors, before);
    }
}
