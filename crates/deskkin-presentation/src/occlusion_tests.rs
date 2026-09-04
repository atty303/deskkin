use super::*;
use crate::{BillboardId, TextureId, WorldUnit, demo_world, raster_billboard_ordered};

fn compare(boards: &[SceneBillboard<'_>], wire: bool) -> SceneStats {
    let stride = 327;
    let mut expected = std::vec![0xdead; stride * 240 + 19];
    let mut actual = expected.clone();
    for y in 0..240 {
        let colors = demo_world::background_row(y, 147);
        for x in 0..320 {
            expected[y * stride + x] = if wire {
                colors[x % 4].to_be()
            } else {
                colors[x % 4]
            };
        }
    }
    let mut baseline_samples = 0;
    for board in boards {
        let stats = raster_billboard_ordered(
            &mut expected,
            stride,
            board.projected,
            board.texture,
            board.region,
            wire,
        )
        .unwrap();
        baseline_samples += stats.nearest_samples + stats.bilinear_samples;
    }
    let stats = raster_scene(
        &mut actual,
        stride,
        boards,
        |y| demo_world::background_row(y, 147),
        wire,
        &mut Occlusion::new(ScreenTile::Eight, &mut [0; 1200]).unwrap(),
    )
    .unwrap();
    assert!(
        actual == expected,
        "tile output or framebuffer guards differ"
    );
    assert!(stats.raster.nearest_samples + stats.raster.bilinear_samples <= baseline_samples);
    assert!(stats.scaler_preparations as usize <= boards.len());
    actual.fill(0xdead);
    raster_scene(
        &mut actual,
        stride,
        boards,
        |y| demo_world::background_row(y, 147),
        wire,
        &mut Occlusion::new(ScreenTile::Sixteen, &mut [0; 300]).unwrap(),
    )
    .unwrap();
    assert!(actual == expected, "16x16 output differs");
    actual.fill(0xdead);
    let mut phases = std::vec::Vec::new();
    raster_scene_observed(
        &mut actual,
        stride,
        boards,
        |y| demo_world::background_row(y, 147),
        wire,
        &mut Occlusion::new(ScreenTile::Eight, &mut [0; 1200]).unwrap(),
        &mut |phase| phases.push(phase),
    )
    .unwrap();
    assert!(actual == expected, "observer changed output");
    assert_eq!(phases.first(), Some(&RasterPhase::Coverage));
    assert_eq!(phases.last(), Some(&RasterPhase::Idle));
    let pixel_phases = phases
        .iter()
        .filter(|&&phase| phase == RasterPhase::Pixels)
        .count();
    assert!(pixel_phases >= stats.scaler_preparations as usize);
    assert!(pixel_phases <= boards.len());
    stats
}

fn board(
    texture: Texture<'_>,
    region: TextureRegion,
    rect: ScreenRect,
    depth: i32,
    id: u16,
    filter: TextureFilter,
) -> SceneBillboard<'_> {
    SceneBillboard::new(
        ProjectedBillboard {
            id: BillboardId(id),
            screen_rect: rect,
            depth: WorldUnit::from_int(depth.try_into().unwrap()),
            source: TextureId(1),
            filter,
        },
        texture,
        region,
    )
    .unwrap()
}

#[test]
fn alternating_opaque_columns_preserve_every_background_span() {
    let texture = Texture {
        size: SourceSize {
            width: 1,
            height: 1,
        },
        pixels: &[0xf81f],
        coverage: Coverage::Opaque,
    };
    let region = TextureRegion {
        source_x: 0,
        source_y: 0,
        width: 1,
        height: 1,
        stride: 1,
    };
    for offset in [0, 8] {
        let boards: std::vec::Vec<_> = (0..20)
            .map(|i| {
                board(
                    texture,
                    region,
                    ScreenRect {
                        x: i * 16 + offset,
                        y: 0,
                        width: 8,
                        height: 240,
                    },
                    1,
                    i as u16,
                    TextureFilter::Nearest,
                )
            })
            .collect();
        for wire in [false, true] {
            compare(&boards, wire);
        }
    }
}

#[test]
fn mask_marks_only_completely_opaque_valid_texels() {
    let size = SourceSize {
        width: 17,
        height: 9,
    };
    let mut alpha = std::vec![255; 17 * 9];
    let mut bits = [0xff];
    build_opaque_mask(size, &alpha, &mut bits).unwrap();
    assert_eq!(bits, [0b0011_1111]);
    alpha[8] = 254;
    alpha[8 * 17 + 16] = 0;
    build_opaque_mask(size, &alpha, &mut bits).unwrap();
    assert_eq!(bits, [0b0001_1101]);
    let unchanged = bits;
    assert_eq!(
        build_opaque_mask(size, &alpha[..5], &mut bits),
        Err(RasterError::InvalidMask)
    );
    assert_eq!(bits, unchanged);
    assert!(Mask8::new(size, &[]).is_err());
    assert_eq!(
        Mask8::bytes_for(SourceSize {
            width: 96,
            height: 96
        }),
        18
    );
}

#[test]
fn occlusion_matches_painter_across_scales_regions_holes_and_ties() {
    let size = SourceSize {
        width: 41,
        height: 29,
    };
    let pixels: std::vec::Vec<u16> = (0..41 * 29)
        .map(|i| (i as u16).wrapping_mul(4919))
        .collect();
    let mut alpha = std::vec![255; pixels.len()];
    let mut bits = std::vec![0; Mask8::bytes_for(size)];
    let region = TextureRegion {
        source_x: 7,
        source_y: 3,
        width: 31,
        height: 23,
        stride: 41,
    };
    let opaque = Texture {
        size,
        pixels: &pixels,
        coverage: Coverage::Opaque,
    };
    let full = ScreenRect {
        x: 0,
        y: 0,
        width: 320,
        height: 240,
    };
    for hole in [None, Some(0), Some(127), Some(254)] {
        alpha.fill(255);
        if let Some(value) = hole {
            for y in 9..17 {
                for x in 9..17 {
                    alpha[y * 41 + x] = value;
                }
            }
        }
        build_opaque_mask(size, &alpha, &mut bits).unwrap();
        let alpha_texture = Texture {
            coverage: Coverage::Alpha8 {
                alpha: &alpha,
                opaque_blocks: Mask8::new(size, &bits).unwrap(),
            },
            ..opaque
        };
        for rect in [
            (0, 0, 320, 240),
            (-5, 13, 31, 23),
            (311, 233, 31, 23),
            (7, 5, 305, 227),
            (-91, -33, 507, 349),
            (311, 234, 19, 7),
            (0, 0, 17, 9),
        ]
        .map(|(x, y, width, height)| ScreenRect {
            x,
            y,
            width,
            height,
        }) {
            for filter in [TextureFilter::Nearest, TextureFilter::Bilinear] {
                let boards = [
                    board(opaque, region, full, 3, 1, TextureFilter::Bilinear),
                    board(alpha_texture, region, rect, 2, 2, filter),
                    board(
                        alpha_texture,
                        region,
                        ScreenRect {
                            x: rect.x + 5,
                            y: rect.y + 7,
                            ..rect
                        },
                        2,
                        3,
                        TextureFilter::Nearest,
                    ),
                ];
                for wire in [false, true] {
                    compare(&boards, wire);
                }
            }
        }
    }
}

#[test]
fn opaque_front_removes_background_and_all_farther_samples() {
    let size = SourceSize {
        width: 1,
        height: 1,
    };
    let texture = Texture {
        size,
        pixels: &[0xf800],
        coverage: Coverage::Opaque,
    };
    let region = TextureRegion {
        source_x: 0,
        source_y: 0,
        width: 1,
        height: 1,
        stride: 1,
    };
    let rect = ScreenRect {
        x: 0,
        y: 0,
        width: 320,
        height: 240,
    };
    let back = board(texture, region, rect, 2, 1, TextureFilter::Bilinear);
    let front = board(texture, region, rect, 1, 2, TextureFilter::Nearest);
    let stats = compare(&[back, front], true);
    assert_eq!(stats.opaque_tiles, 1200);
    assert_eq!(stats.skipped_background_pixels, 76800);
    assert_eq!(stats.raster.nearest_samples, 76800);
    assert_eq!(stats.raster.bilinear_samples, 0);
    assert_eq!(stats.scaler_preparations, 1);
    let mut storage = [0; 1200];
    let mut map = Occlusion::new(ScreenTile::Eight, &mut storage).unwrap();
    let mut reused = std::vec![0; 76800];
    raster_scene(
        &mut reused,
        320,
        &[back, front],
        |_| [123; 4],
        false,
        &mut map,
    )
    .unwrap();
    let empty = raster_scene(&mut reused, 320, &[], |_| [123; 4], false, &mut map).unwrap();
    assert_eq!(empty.opaque_tiles, 0);
    assert!(reused.iter().all(|&pixel| pixel == 123));
    assert!(Occlusion::new(ScreenTile::Sixteen, &mut storage).is_err());
    let mut output = std::vec![0; 76800];
    assert_eq!(
        raster_scene(
            &mut output,
            320,
            &[front, back],
            |_| [0; 4],
            false,
            &mut Occlusion::new(ScreenTile::Eight, &mut [0; 1200]).unwrap()
        )
        .unwrap_err(),
        RasterError::InvalidOrder
    );
    assert!(output.iter().all(|&p| p == 0));
    compare(&[], false);
}

#[test]
fn sparse_and_overdraw_timing_samples() {
    let size = SourceSize {
        width: 32,
        height: 32,
    };
    let pixels: std::vec::Vec<u16> = (0..1024).map(|i| (i as u16).wrapping_mul(4937)).collect();
    let alpha_plane: std::vec::Vec<u8> = (0..1024)
        .map(|i| if i % 5 == 0 { 0 } else { 192 })
        .collect();
    let mut bits = std::vec![0; Mask8::bytes_for(size)];
    build_opaque_mask(size, &alpha_plane, &mut bits).unwrap();
    for alpha in [false, true] {
        let texture = Texture {
            size,
            pixels: &pixels,
            coverage: if alpha {
                Coverage::Alpha8 {
                    alpha: &alpha_plane,
                    opaque_blocks: Mask8::new(size, &bits).unwrap(),
                }
            } else {
                Coverage::Opaque
            },
        };
        let region = TextureRegion {
            source_x: 0,
            source_y: 0,
            width: 32,
            height: 32,
            stride: 32,
        };
        for overlap in [false, true] {
            let boards: std::vec::Vec<_> = (0..12)
                .map(|i| {
                    board(
                        texture,
                        region,
                        if overlap {
                            ScreenRect {
                                x: i * 3,
                                y: i * 2,
                                width: 260,
                                height: 180,
                            }
                        } else {
                            ScreenRect {
                                x: (i % 4) * 80,
                                y: (i / 4) * 80,
                                width: 60,
                                height: 60,
                            }
                        },
                        12 - i,
                        i as u16,
                        if alpha {
                            TextureFilter::Nearest
                        } else {
                            TextureFilter::Bilinear
                        },
                    )
                })
                .collect();
            measure_scene(&boards, overlap, alpha);
        }
    }
}

fn measure_scene(boards: &[SceneBillboard<'_>], overlap: bool, alpha: bool) {
    compare(boards, false);
    let mut output = std::vec![0; 76800];
    let start = std::time::Instant::now();
    for _ in 0..8 {
        for y in 0..240 {
            let colors = demo_world::background_row(y, 147);
            for row in output[y * 320..(y + 1) * 320].chunks_exact_mut(4) {
                row.copy_from_slice(&colors);
            }
        }
        for b in boards {
            raster_billboard_ordered(&mut output, 320, b.projected, b.texture, b.region, false)
                .unwrap();
        }
        std::hint::black_box(&output);
    }
    let painter = start.elapsed();
    for tile in [ScreenTile::Eight, ScreenTile::Sixteen] {
        let mut storage = std::vec![0; tile.cells()];
        let mut occlusion = Occlusion::new(tile, &mut storage).unwrap();
        let start = std::time::Instant::now();
        let mut stats = SceneStats::default();
        for _ in 0..8 {
            stats = raster_scene(
                &mut output,
                320,
                boards,
                |y| demo_world::background_row(y, 147),
                false,
                &mut occlusion,
            )
            .unwrap();
            std::hint::black_box(&output);
        }
        std::println!(
            "host scene sample overlap={overlap} alpha={alpha} tile={tile:?}: painter={painter:?}, tiles={:?}, stats={stats:?}",
            start.elapsed()
        );
    }
}

#[test]
fn native_sprites_do_not_prepare_scalers() {
    let size = SourceSize {
        width: 3,
        height: 3,
    };
    let texture = Texture {
        size,
        pixels: &[0xffff; 9],
        coverage: Coverage::Opaque,
    };
    let region = TextureRegion {
        source_x: 0,
        source_y: 0,
        width: 3,
        height: 3,
        stride: 3,
    };
    let rect = ScreenRect {
        x: 10,
        y: 10,
        width: 3,
        height: 3,
    };
    let native = board(texture, region, rect, 1, 1, TextureFilter::Nearest);
    assert_eq!(compare(&[native], true).scaler_preparations, 0);
    let scaled = board(
        texture,
        region,
        ScreenRect {
            width: 6,
            height: 6,
            ..rect
        },
        1,
        1,
        TextureFilter::Nearest,
    );
    assert_eq!(compare(&[scaled], true).scaler_preparations, 1);
}
