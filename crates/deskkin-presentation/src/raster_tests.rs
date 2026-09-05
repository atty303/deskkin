use super::*;

const REGIONS: [TextureRegion; 3] = [
    TextureRegion {
        source_x: 0,
        source_y: 0,
        width: 41,
        height: 29,
        stride: 41,
    },
    TextureRegion {
        source_x: 7,
        source_y: 3,
        width: 31,
        height: 23,
        stride: 41,
    },
    TextureRegion {
        source_x: 40,
        source_y: 28,
        width: 1,
        height: 1,
        stride: 41,
    },
];

fn reference_mix(first: u16, second: u16, fraction: u32) -> u16 {
    let mut result = 0;
    for (shift, mask) in [(11, 31), (5, 63), (0, 31)] {
        let a = u32::from((first >> shift) & mask);
        let b = u32::from((second >> shift) & mask);
        result |= (((a * (65_536 - fraction) + b * fraction + 32_768) >> 16) as u16) << shift;
    }
    result
}

// Deliberately direct division and per-pixel addressing: an independent oracle
// for incremental stepping, specialization and skipped alpha endpoint work.
fn reference(
    framebuffer: &mut [u16],
    stride: usize,
    board: ProjectedBillboard,
    texture: Texture<'_>,
    region: TextureRegion,
    wire: bool,
) -> RasterStats {
    let rect = board.screen_rect;
    let mut stats = RasterStats::default();
    for y in rect.y.max(0)..rect.y.saturating_add(rect.height).min(VIEWPORT_HEIGHT) {
        for x in rect.x.max(0)..rect.x.saturating_add(rect.width).min(VIEWPORT_WIDTH) {
            let sx = (i64::from(x) - i64::from(rect.x)) * i64::from(region.width) * 65_536
                / i64::from(rect.width);
            let sy = (i64::from(y) - i64::from(rect.y)) * i64::from(region.height) * 65_536
                / i64::from(rect.height);
            let x0 = (sx >> 16) as usize + usize::from(region.source_x);
            let y0 = (sy >> 16) as usize + usize::from(region.source_y);
            let index = y0 * usize::from(region.stride) + x0;
            let (color, alpha_index) = match board.filter {
                TextureFilter::Nearest => {
                    stats.nearest_samples += 1;
                    (texture.pixels[index], index)
                }
                TextureFilter::Bilinear => {
                    stats.bilinear_samples += 1;
                    let x1 = (x0 + 1).min(usize::from(region.source_x + region.width - 1));
                    let y1 = (y0 + 1).min(usize::from(region.source_y + region.height - 1));
                    let top = reference_mix(
                        texture.pixels[index],
                        texture.pixels[y0 * usize::from(region.stride) + x1],
                        (sx & 0xffff) as u32,
                    );
                    let bottom = reference_mix(
                        texture.pixels[y1 * usize::from(region.stride) + x0],
                        texture.pixels[y1 * usize::from(region.stride) + x1],
                        (sx & 0xffff) as u32,
                    );
                    (reference_mix(top, bottom, (sy & 0xffff) as u32), 0)
                }
            };
            let destination = &mut framebuffer[y as usize * stride + x as usize];
            let background = if wire {
                u16::from_be(*destination)
            } else {
                *destination
            };
            let output = if texture.coverage.is_alpha() && board.filter == TextureFilter::Bilinear {
                let x1 = (x0 + 1).min(usize::from(region.source_x + region.width - 1));
                let y1 = (y0 + 1).min(usize::from(region.source_y + region.height - 1));
                let indices = [
                    index,
                    y0 * usize::from(region.stride) + x1,
                    y1 * usize::from(region.stride) + x0,
                    y1 * usize::from(region.stride) + x1,
                ];
                let weights = indices.map(|i| u64::from(texture.coverage.at(i)));
                let mix = |p: [u64; 4]| {
                    let f = (sx & 0xffff) as u64;
                    let g = (sy & 0xffff) as u64;
                    let top = (p[0] * (65536 - f) + p[1] * f + 32768) >> 16;
                    let bottom = (p[2] * (65536 - f) + p[3] * f + 32768) >> 16;
                    (top * (65536 - g) + bottom * g + 32768) >> 16
                };
                let alpha = mix(weights);
                let mut pixel = 0;
                for (shift, mask) in [(11, 31u16), (5, 63), (0, 31)] {
                    let values = core::array::from_fn(|i| {
                        u64::from((texture.pixels[indices[i]] >> shift) & mask) * weights[i]
                    });
                    let channel = ((mix(values)
                        + u64::from((background >> shift) & mask) * (255 - alpha)
                        + 127)
                        / 255)
                        .min(u64::from(mask));
                    pixel |= (channel as u16) << shift;
                }
                pixel
            } else if texture.coverage.is_alpha() {
                reference_mix(
                    background,
                    color,
                    u32::from(texture.coverage.alpha()[alpha_index]) * 257,
                )
            } else {
                color
            };
            *destination = if wire { output.to_be() } else { output };
        }
    }
    stats
}

#[test]
fn incremental_coordinates_equal_division_at_all_visible_samples() {
    for source in [1, 2, 3, 31, 136, 272, 1023, u16::MAX] {
        for destination in [1, 2, 3, 7, 239, 320, 511, 65_535, i32::MAX] {
            for start in [0, destination / 3, (destination - 320).max(0)] {
                let mut axis = AxisStepper::new(source, destination, i64::from(start));
                for offset in start..destination.min(start.saturating_add(320)) {
                    assert_eq!(
                        u64::from(axis.take()),
                        (offset as u64 * u64::from(source) * 65_536) / destination as u64
                    );
                }
            }
        }
    }
}

#[test]
fn specialized_raster_matches_reference_pixels_stats_and_guards() {
    let stride = 327;
    let initial: std::vec::Vec<u16> = (0..stride * 240 + 17)
        .map(|i| (i as u16).wrapping_mul(719).wrapping_add(13))
        .collect();
    let pixels: std::vec::Vec<u16> = (0..41 * 29)
        .map(|i| (i as u16).wrapping_mul(3571).wrapping_add(193))
        .collect();
    let mut alpha: std::vec::Vec<u8> = (0..pixels.len())
        .map(|i| match i % 5 {
            0 => 0,
            1 => 255,
            _ => i as u8,
        })
        .collect();
    let mut reference_time = std::time::Duration::ZERO;
    let mut optimized_time = std::time::Duration::ZERO;
    let mut expected = initial.clone();
    let mut actual = initial.clone();
    for region in REGIONS {
        for (x, y, width, height) in [
            (0, 0, 320, 240),
            (-83, -71, 397, 317),
            (299, 229, 57, 39),
            (41, 29, 17, 11),
            (0, 0, 1, 1),
            (0, 0, 41, 29),
            (-5, -7, i32::from(region.width), i32::from(region.height)),
            (311, 233, i32::from(region.width), i32::from(region.height)),
            (-320, 0, 320, 240),
            (320, 0, 100, 100),
            (0, 240, 100, 100),
            (-i32::MAX + 100, -i32::MAX + 100, i32::MAX, i32::MAX),
        ] {
            for filter in [TextureFilter::Nearest, TextureFilter::Bilinear] {
                for format in [false, true] {
                    for wire in [false, true] {
                        for first_alpha in [0, 127, 255] {
                            alpha[0] = first_alpha;
                            let size = SourceSize {
                                width: 41,
                                height: 29,
                            };
                            let mut bits = std::vec![0; Mask8::bytes_for(size)];
                            build_opaque_mask(size, &alpha, &mut bits).unwrap();
                            let board = ProjectedBillboard {
                                id: BillboardId(7),
                                screen_rect: ScreenRect {
                                    x,
                                    y,
                                    width,
                                    height,
                                },
                                depth: WorldUnit::ONE,
                                source: TextureId(9),
                                filter,
                            };
                            let texture = Texture {
                                size,
                                pixels: &pixels,
                                coverage: if format {
                                    Coverage::Alpha8 {
                                        alpha: &alpha,
                                        opaque_blocks: Mask8::new(size, &bits).unwrap(),
                                    }
                                } else {
                                    Coverage::Opaque
                                },
                            };
                            expected.copy_from_slice(&initial);
                            actual.copy_from_slice(&initial);
                            let started = std::time::Instant::now();
                            let expected_stats =
                                reference(&mut expected, stride, board, texture, region, wire);
                            reference_time += started.elapsed();
                            let started = std::time::Instant::now();
                            let actual_stats = raster_billboard_ordered(
                                &mut actual,
                                stride,
                                board,
                                texture,
                                region,
                                wire,
                            )
                            .unwrap();
                            optimized_time += started.elapsed();
                            assert_eq!(actual_stats, expected_stats);
                            assert!(
                                actual == expected,
                                "{region:?} {filter:?} {format:?} wire={wire} rect={:?}",
                                board.screen_rect
                            );
                        }
                    }
                }
            }
        }
    }
    std::println!(
        "host raster sample (not a device benchmark): reference={reference_time:?}, optimized={optimized_time:?}"
    );
}

#[test]
fn packed_coverage_matches_a8_across_clips_scaling_and_padding() {
    let size = SourceSize {
        width: 48,
        height: 17,
    };
    let pixels: std::vec::Vec<_> = (0..48 * 17).map(|i: u16| i.wrapping_mul(971)).collect();
    let alpha: std::vec::Vec<_> = (0..pixels.len())
        .map(|i| if (i * 79) % 13 < 7 { 255 } else { 0 })
        .collect();
    let mut blocks = std::vec![0; Mask8::bytes_for(size)];
    build_opaque_mask(size, &alpha, &mut blocks).unwrap();
    let mut bits = std::vec![0; alpha.len().div_ceil(8)];
    build_cutout_mask(&alpha, &mut bits).unwrap();
    let mask = Mask8::new(size, &blocks).unwrap();
    for wire in [false, true] {
        for filter in [TextureFilter::Nearest, TextureFilter::Bilinear] {
            for x in [-37, 0, 13, 309] {
                for (width, height) in [(37, 13), (317, 153), (11, 7)] {
                    let board = ProjectedBillboard {
                        id: BillboardId(1),
                        source: TextureId(1),
                        depth: WorldUnit::ONE,
                        filter,
                        screen_rect: ScreenRect {
                            x,
                            y: -3,
                            width,
                            height,
                        },
                    };
                    let region = TextureRegion {
                        source_x: 5,
                        source_y: 2,
                        width: 37,
                        height: 13,
                        stride: 48,
                    };
                    let mut expected = std::vec![0x965a; 327 * 240 + 19];
                    let mut actual = expected.clone();
                    for (buffer, coverage) in [
                        (
                            &mut expected,
                            Coverage::Alpha8 {
                                alpha: &alpha,
                                opaque_blocks: mask,
                            },
                        ),
                        (
                            &mut actual,
                            Coverage::Cutout {
                                bits: &bits,
                                opaque_blocks: mask,
                            },
                        ),
                    ] {
                        raster_billboard_ordered(
                            buffer,
                            327,
                            board,
                            Texture {
                                size,
                                pixels: &pixels,
                                coverage,
                            },
                            region,
                            wire,
                        )
                        .unwrap();
                    }
                    assert_eq!(actual, expected);
                }
            }
        }
    }
    let mut sentinel = [0xa5];
    assert!(build_cutout_mask(&[0, 127, 255], &mut sentinel).is_err());
    assert_eq!(sentinel, [0xa5]);
}
