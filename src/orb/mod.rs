use std::f32::consts::TAU;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

const SIZE: u32 = 640;
const CENTER: f32 = SIZE as f32 / 2.0;
const RADIUS: f32 = 202.0;
const PARTICLE_COUNT: usize = 5_200;
const GOLDEN_ANGLE: f32 = 2.399_963_1;

type Color = [u8; 3];

struct Palette {
    halo: Color,
    body_light: Color,
    body_dark: Color,
    guide: Color,
    marker: Color,
    particle_dark: [Color; 2],
    particle_light: [Color; 2],
    band: Color,
}

const TEAL: Palette = Palette {
    halo: [161, 215, 221],
    body_light: [236, 254, 249],
    body_dark: [78, 172, 187],
    guide: [125, 171, 180],
    marker: [106, 156, 163],
    particle_dark: [[13, 82, 112], [9, 115, 119]],
    particle_light: [[44, 176, 196], [156, 247, 229]],
    band: [55, 187, 181],
};

const PURPLE: Palette = Palette {
    halo: [200, 163, 236],
    body_light: [247, 236, 255],
    body_dark: [144, 77, 204],
    guide: [166, 132, 209],
    marker: [143, 104, 196],
    particle_dark: [[68, 30, 119], [101, 44, 151]],
    particle_light: [[154, 85, 217], [235, 192, 255]],
    band: [171, 100, 227],
};

struct Particle {
    x: f32,
    y: f32,
    z: f32,
    longitude: f32,
    seed: f32,
}

#[derive(Clone, Copy, Default)]
pub struct PointerMotion {
    x: f32,
    y: f32,
    strength: f32,
}

impl PointerMotion {
    pub fn update(&mut self, position: Option<(f32, f32)>) -> bool {
        if let Some((x, y)) = position {
            self.x += (x - self.x) * 0.42;
            self.y += (y - self.y) * 0.42;
            self.strength += (1.0 - self.strength) * 0.38;
            return true;
        }
        if self.strength == 0.0 {
            return false;
        }
        self.strength *= 0.70;
        if self.strength < 0.01 {
            *self = Self::default();
        }
        true
    }
}

pub struct Orb {
    teal_backdrop: SharedPixelBuffer<Rgba8Pixel>,
    purple_backdrop: SharedPixelBuffer<Rgba8Pixel>,
    particles: Vec<Particle>,
}

impl Orb {
    pub fn new() -> Self {
        let particles = (0..PARTICLE_COUNT)
            .map(|index| {
                let y = 1.0 - 2.0 * (index as f32 + 0.5) / PARTICLE_COUNT as f32;
                let longitude = (index as f32 * GOLDEN_ANGLE) % TAU;
                let (sin, cos) = longitude.sin_cos();
                let width = (1.0 - y * y).sqrt();
                let seed = (index.wrapping_mul(7919) % 4093) as f32 / 4093.0;
                Particle {
                    x: width * cos,
                    y,
                    z: width * sin,
                    longitude,
                    seed,
                }
            })
            .collect();

        Self {
            teal_backdrop: make_backdrop(&TEAL),
            purple_backdrop: make_backdrop(&PURPLE),
            particles,
        }
    }

    pub fn frame(&self, time: f32, level: f32, thinking: bool, pointer: PointerMotion) -> Image {
        let (backdrop, palette) = if thinking {
            (&self.purple_backdrop, &PURPLE)
        } else {
            (&self.teal_backdrop, &TEAL)
        };
        let mut buffer = backdrop.clone();
        let pixels = buffer.make_mut_slice();
        let (sin, cos) = (time * 0.20 + 0.35 + pointer.x * pointer.strength * 0.40).sin_cos();
        let (tilt_sin, tilt_cos) = (-0.34 - pointer.y * pointer.strength * 0.25).sin_cos();
        let center_x = CENTER + pointer.x * pointer.strength * 8.0;
        let center_y = CENTER + pointer.y * pointer.strength * 8.0;
        let radius = RADIUS * (1.0 + 0.012 * (time * 1.65).sin() + 0.065 * level);

        for particle in &self.particles {
            let x = particle.x * cos + particle.z * sin;
            let before_tilt = particle.z * cos - particle.x * sin;
            let y = particle.y * tilt_cos - before_tilt * tilt_sin;
            let z = particle.y * tilt_sin + before_tilt * tilt_cos;
            let ripple = (particle.longitude * 4.0 + particle.y * 9.0 - time * 2.5).sin();
            let wave = (particle.longitude * 2.0 - particle.y * 7.0 + time * 1.15).cos();
            let front = smoothstep(-0.24, 0.91, z);
            let (pull, hover_wave) = if pointer.strength > 0.0 {
                let from_pointer = (x - pointer.x).hypot(-y - pointer.y);
                let focus = 1.0 - smoothstep(0.05, 0.92, from_pointer);
                let pulse = (from_pointer * 15.0 - time * 8.0).sin();
                (focus * front * pointer.strength, pulse)
            } else {
                (0.0, 0.0)
            };
            let distance = radius
                + (3.8 + 14.0 * level) * ripple
                + (2.6 + 4.0 * level) * wave
                + (7.0 + 13.0 * hover_wave) * pull;

            let lit = smoothstep(-0.54, 0.80, -0.56 * x + 0.48 * y + 0.18 * z);
            let glint = smoothstep(0.63, 0.96, ripple * 0.5 + 0.5) * front;
            let dark = mix(palette.particle_dark[0], palette.particle_dark[1], lit);
            let bright = mix(palette.particle_light[0], palette.particle_light[1], lit);
            let shine = glint * 0.74 + particle.seed * 0.26 + pull * (0.42 + 0.22 * hover_wave);
            let color = mix(dark, bright, shine.min(1.0));
            let opacity = (0.27 + 0.60 * front) * (0.64 + 0.36 * particle.seed);
            let size = 0.68 + 0.74 * particle.seed + 0.30 * front + 0.36 * level + 0.50 * pull;

            spot(
                pixels,
                center_x + x * distance,
                center_y - y * distance,
                size,
                color,
                opacity,
            );
        }

        for band in 0..3 {
            let band = band as f32;
            for index in 0..320 {
                let theta = index as f32 / 320.0 * TAU;
                let (around_x, around_z) = (theta + time * 0.18).sin_cos();
                let ripple = (theta * 3.0 - time * 2.4 + band * 0.8).sin();
                let y = -0.26 + band * 0.18 + 0.22 * around_x + (0.045 + 0.042 * level) * ripple;
                let width = (1.0 - y * y).sqrt();
                let z = around_z * width;
                let x = around_x * width;
                let dist = radius + 7.0 + 8.0 * level + 3.0 * ripple;
                let opacity = (0.2 + 0.65 * smoothstep(-0.25, 0.85, z)) * (0.77 + level * 0.23);
                spot(
                    pixels,
                    center_x + x * dist,
                    center_y - (y * tilt_cos - z * tilt_sin) * dist,
                    0.83 + 0.33 * level,
                    palette.band,
                    opacity,
                );
            }
        }

        Image::from_rgba8(buffer)
    }
}

fn make_backdrop(palette: &Palette) -> SharedPixelBuffer<Rgba8Pixel> {
    let mut buffer = SharedPixelBuffer::new(SIZE, SIZE);
    for (index, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let x = (index as u32 % SIZE) as f32 + 0.5 - CENTER;
        let y = (index as u32 / SIZE) as f32 + 0.5 - CENTER;
        let distance = x.hypot(y);
        let rim = distance / RADIUS;

        let halo = (-distance * distance / 39_000.0).exp() * 0.18;
        let shadow_y = (y - 233.0) * 5.5;
        let shadow = (-(x * x + shadow_y * shadow_y) / 8_000.0).exp() * 0.22;
        let mut color = Rgba8Pixel::default();
        composite(&mut color, palette.halo, halo + shadow);

        let light = smoothstep(-0.9, 1.1, (x * 0.53 + y * 0.70) / RADIUS);
        let at_edge = smoothstep(0.45, 1.0, rim);
        let body = mix(
            palette.body_light,
            palette.body_dark,
            light * 0.61 + at_edge * 0.23,
        );
        composite(&mut color, body, 1.0 - smoothstep(0.92, 1.064, rim));

        let inner_ring = 1.0 - smoothstep(0.1, 1.3, (distance - 240.0).abs());
        let outer_ring = 1.0 - smoothstep(0.1, 1.4, (distance - 285.0).abs());
        let near_tick_band = (275.0..=293.0).contains(&distance);
        let ticks = if near_tick_band {
            let turn = y.atan2(x) / TAU;
            let fraction = (turn * 80.0).abs().fract();
            let angle = fraction.min(1.0 - fraction);
            let lengths = if (turn * 20.0).round().mul_add(-4.0, turn * 80.0).abs() < 0.5 {
                8.0
            } else {
                3.6
            };
            (1.0 - smoothstep(0.015, 0.059, angle))
                * (1.0 - smoothstep(lengths - 0.8, lengths, (distance - 285.0).abs()))
        } else {
            0.0
        };
        composite(
            &mut color,
            palette.guide,
            inner_ring * 0.25 + outer_ring * 0.22 + ticks * 0.22,
        );
        *pixel = color;
    }

    let pixels = buffer.make_mut_slice();
    for (x, y) in [(-285.0, 0.0), (285.0, 0.0), (0.0, -285.0), (0.0, 285.0)] {
        spot(pixels, CENTER + x, CENTER + y, 2.6, palette.marker, 0.75);
    }
    buffer
}

fn spot(pixels: &mut [Rgba8Pixel], x: f32, y: f32, size: f32, color: Color, opacity: f32) {
    let reach = size + 0.6;
    let reach_squared = reach * reach;
    let left = ((x - reach - 0.5).ceil() as i32).max(0);
    let right = ((x + reach - 0.5).floor() as i32).min(SIZE as i32 - 1);
    let top = ((y - reach - 0.5).ceil() as i32).max(0);
    let bottom = ((y + reach - 0.5).floor() as i32).min(SIZE as i32 - 1);

    for row in top..=bottom {
        let dy = row as f32 + 0.5 - y;
        for col in left..=right {
            let dx = col as f32 + 0.5 - x;
            let distance_squared = dx * dx + dy * dy;
            if distance_squared < reach_squared {
                let coverage = (reach - distance_squared.sqrt()).min(1.0) * opacity;
                let pixel = &mut pixels[row as usize * SIZE as usize + col as usize];
                composite(pixel, color, coverage);
            }
        }
    }
}

fn composite(pixel: &mut Rgba8Pixel, color: Color, coverage: f32) {
    let previous = f32::from(pixel.a) / 255.0 * (1.0 - coverage);
    let alpha = previous + coverage;
    if alpha > 0.0 {
        let [r, g, b] = mix([pixel.r, pixel.g, pixel.b], color, coverage / alpha);
        *pixel = Rgba8Pixel {
            r,
            g,
            b,
            a: (alpha * 255.0).round() as u8,
        };
    }
}

fn mix(from: Color, to: Color, amount: f32) -> Color {
    std::array::from_fn(|index| {
        let from = from[index] as f32;
        let to = to[index] as f32;
        (from + (to - from) * amount).round() as u8
    })
}

fn smoothstep(start: f32, end: f32, value: f32) -> f32 {
    let mix = ((value - start) / (end - start)).clamp(0.0, 1.0);
    mix * mix * (3.0 - 2.0 * mix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stronger_input_expands_the_orb() {
        let orb = Orb::new();
        let pointer = PointerMotion::default();
        let at_rest = orb.frame(0.0, 0.0, false, pointer).to_rgba8().unwrap();
        let loud = orb.frame(0.0, 1.0, false, pointer).to_rgba8().unwrap();
        let outer_pixels = |frame: &SharedPixelBuffer<Rgba8Pixel>| {
            frame
                .as_slice()
                .iter()
                .enumerate()
                .filter(|(index, pixel)| {
                    let x = (*index as u32 % SIZE) as f32 - CENTER;
                    let y = (*index as u32 / SIZE) as f32 - CENTER;
                    let red_on_white = 255 - u16::from(255 - pixel.r) * u16::from(pixel.a) / 255;
                    x * x + y * y > (RADIUS + 12.0).powi(2) && red_on_white < 165
                })
                .count()
        };
        assert!(outer_pixels(&loud) > outer_pixels(&at_rest) + 200);
    }

    #[test]
    fn thinking_switches_the_orb_from_teal_to_purple_without_audio() {
        let orb = Orb::new();
        let pointer = PointerMotion::default();
        let teal = orb.frame(0.0, 0.0, false, pointer).to_rgba8().unwrap();
        let purple = orb.frame(0.0, 0.0, true, pointer).to_rgba8().unwrap();
        let center = (SIZE * (SIZE / 2) + SIZE / 2) as usize;
        let teal_pixel = teal.as_slice()[center];
        let purple_pixel = purple.as_slice()[center];
        assert!(teal_pixel.g > teal_pixel.r);
        assert!(purple_pixel.r > purple_pixel.g && purple_pixel.b > purple_pixel.r);
    }

    #[test]
    fn pointer_animates_the_silent_orb_and_settles_after_leaving() {
        let orb = Orb::new();
        let mut pointer = PointerMotion::default();
        let idle = orb.frame(0.0, 0.0, false, pointer).to_rgba8().unwrap();
        assert!(!pointer.update(None));
        assert!(pointer.update(Some((0.65, -0.30))));
        let hovered = orb.frame(0.0, 0.0, false, pointer).to_rgba8().unwrap();
        assert_ne!(idle.as_slice(), hovered.as_slice());
        assert!(pointer.update(None));
        while pointer.update(None) {}
        let settled = orb.frame(0.0, 0.0, false, pointer).to_rgba8().unwrap();
        assert_eq!(idle.as_slice(), settled.as_slice());
    }
}
