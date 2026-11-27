use super::native::Zone;
use crate::core::{Resource, Vector};
use macroquad::prelude::*;
use nalgebra::Rotation2;

fn corners(res: &Resource, zone: &Zone) -> [Vec2; 4] {
    let [sx, sy] = res.block_area_chart_scale;
    let fx = if res.config.flip_x() { -1.0 } else { 1.0 };
    let fy = if res.config.flip_chart { -1.0 } else { 1.0 };
    let mirror = res.mirror_angle();
    let rotation = Rotation2::new(zone.angle);
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| {
        let p = zone.center + rotation * zone.half.component_mul(&Vector::new(x, y));
        let p = Vector::new(fx * p.x, fy * p.y);
        let p = mirror.map_or(p, |angle| Resource::mirror_vector(-angle, p));
        vec2(p.x * sx, p.y * sy)
    })
}

pub(in crate::core) fn draw_individual(res: &Resource, zones: impl IntoIterator<Item = Zone>) {
    let alpha = res.config.block_area_reveal_alpha.clamp(0.0, 1.0);
    let [r, g, b] = res.config.block_area_normal_color;
    let thickness = 4.0 / res.last_vp.2.max(1) as f32;
    for zone in zones {
        let corners = corners(res, &zone);
        let intensity = if zone.active { 1.0 } else { 0.55 };
        let fill = Color::new(r * intensity, g * intensity, b * intensity, alpha);
        draw_triangle(corners[0], corners[1], corners[2], fill);
        draw_triangle(corners[0], corners[2], corners[3], fill);
        if res.config.block_area_reveal_outline {
            for (a, b) in corners.iter().zip(corners.iter().cycle().skip(1)) {
                draw_line(a.x, a.y, b.x, b.y, thickness, WHITE);
            }
        }
    }
}

/// Rasterize geometric coverage before applying display opacity or outlining.
pub(in crate::core) fn combine(
    output: &mut [u8],
    width: usize,
    height: usize,
    res: &Resource,
    zones: &[Zone],
    single_inversion: bool,
) {
    if zones.is_empty() {
        return;
    }
    let stride = width + 1;
    let mut differences = vec![[0_i32; 4]; stride * height];
    for zone in zones {
        let corners = corners(res, zone).map(|p| {
            vec2((p.x + 1.) * width as f32 * 0.5, (p.y * res.aspect_ratio + 1.) * height as f32 * 0.5)
        });
        let channel = usize::from(zone.active) * 2 + usize::from(zone.invert);
        let row = |y: f32| (y - 0.5).ceil().clamp(0., height as f32) as usize;
        let first = row(corners.iter().map(|p| p.y).fold(f32::INFINITY, f32::min));
        let last = row(corners.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max));
        for y in first..last {
            let py = y as f32 + 0.5;
            let (mut left, mut right) = (f32::INFINITY, f32::NEG_INFINITY);
            for (a, b) in corners.iter().zip(corners.iter().cycle().skip(1)) {
                if (a.y > py) != (b.y > py) {
                    let x = a.x + (b.x - a.x) * (py - a.y) / (b.y - a.y);
                    left = left.min(x);
                    right = right.max(x);
                }
            }
            let column = |x: f32| (x - 0.5).ceil().clamp(0., width as f32) as usize;
            if left < right {
                differences[y * stride + column(left)][channel] += 1;
                differences[y * stride + column(right)][channel] -= 1;
            }
        }
    }
    for (row, out) in differences.chunks_exact(stride).zip(output.chunks_exact_mut(width)) {
        let mut counts = [0; 4];
        for (delta, state) in row.iter().zip(out) {
            for (count, value) in counts.iter_mut().zip(delta) {
                *count += value;
            }
            *state = (*state).max(coverage(counts, single_inversion));
        }
    }
}

fn coverage([idle, idle_inverse, active, active_inverse]: [i32; 4], single: bool) -> u8 {
    let inverted = |count| if single { count == 1 } else { count % 2 == 1 };
    if active != 0 || active_inverse != 0 {
        if (active > 0) ^ inverted(active_inverse) { 2 } else { 0 }
    } else {
        u8::from((idle > 0) ^ inverted(idle_inverse))
    }
}

pub(in crate::core) fn outline(
    state: &[u8], width: usize, height: usize, [r, g, b]: [f32; 3], opacity: f32, draw_outline: bool,
) -> Vec<u8> {
    let mut bytes = vec![0; state.len() * 4];
    for (i, (&value, pixel)) in state.iter().zip(bytes.chunks_exact_mut(4)).enumerate() {
        if value == 0 {
            continue;
        }
        let x = i % width;
        let y = i / width;
        if draw_outline && (x == 0 || x + 1 == width || y == 0 || y + 1 == height
            || state[i - 1] == 0 || state[i + 1] == 0 || state[i - width] == 0 || state[i + width] == 0)
        {
            pixel.copy_from_slice(&[255; 4]);
        } else {
            let intensity = if value == 2 { 255.0 } else { 255.0 * 0.55 };
            pixel.copy_from_slice(&[(r * intensity) as u8, (g * intensity) as u8, (b * intensity) as u8, (opacity * 255.0).round() as u8]);
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtractive_overlap_is_transparent_and_outlined() {
        assert_eq!(coverage([0, 0, 1, 0], true), 2);
        assert_eq!(coverage([0, 0, 1, 1], true), 0);
        assert_eq!(coverage([0, 0, 1, 2], true), 2);
        assert_eq!(coverage([0, 0, 1, 3], true), 2);
        assert_eq!(coverage([0, 0, 1, 3], false), 0);
        let mut state = vec![2; 25];
        state[12] = 0;
        let rgba = outline(&state, 5, 5, [1., 0., 0.], 0.5, true);
        assert_eq!(&rgba[48..52], &[0; 4]);
        assert_eq!(&rgba[44..48], &[255; 4]);
        assert_eq!(&rgba[24..28], &[255, 0, 0, 128]);
        let rgba = outline(&state, 5, 5, [1., 0., 0.], 0.5, false);
        assert_eq!(&rgba[48..52], &[0; 4]);
        assert_eq!(&rgba[44..48], &[255, 0, 0, 128]);
    }
}
