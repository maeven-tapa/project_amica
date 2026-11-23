use std::{f32::consts::TAU, fmt::Write};

struct EdgePoint {
    x: f32,
    y: f32,
    nx: f32,
    ny: f32,
    phase: f32,
    ripple_weight: f32,
}

#[derive(Default)]
pub(crate) struct RippleBorder {
    size: (f32, f32),
    perimeter: Vec<EdgePoint>,
    strength: f32,
    phase: f32,
}

impl RippleBorder {
    pub(super) fn update(
        &mut self,
        width: f32,
        height: f32,
        speech: f32,
        elapsed: f32,
    ) -> Option<(String, f32)> {
        let was_active = self.strength > 0.0;
        let time_step = elapsed / 0.04;
        let retention: f32 = if speech > self.strength { 0.48 } else { 0.72 };
        let response = 1.0 - retention.powf(time_step);
        self.strength += (speech - self.strength) * response;
        if self.strength < 0.018 {
            self.strength = 0.0;
            self.phase = 0.0;
            return was_active.then(|| (String::new(), 0.0));
        }
        self.phase = (self.phase + (0.16 + self.strength * 0.12) * time_step) % TAU;
        let ripple = self.path(width, height, self.phase, self.strength);
        Some((ripple, self.strength))
    }

    pub(crate) fn path(&mut self, width: f32, height: f32, phase: f32, strength: f32) -> String {
        if self.size != (width, height) {
            self.size = (width, height);
            self.perimeter = perimeter(width, height);
        }

        let mut ripple = String::with_capacity(self.perimeter.len() * 13);
        for (index, point) in self.perimeter.iter().enumerate() {
            let wave = point.phase * 4.0 - phase;
            let ripple_detail = (point.phase * 11.0 - 2.0 * phase).sin();
            let inward =
                strength * point.ripple_weight * (3.8 + 2.6 * wave.sin() + 0.8 * ripple_detail);
            let x = point.x - point.nx * inward;
            let y = point.y - point.ny * inward;
            let command = if index == 0 { 'M' } else { 'L' };
            write!(ripple, "{command}{x:.1} {y:.1}").unwrap();
        }
        if !ripple.is_empty() {
            ripple.push('Z');
        }
        ripple
    }
}

fn perimeter(width: f32, height: f32) -> Vec<EdgePoint> {
    if width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    let length = 2.0 * (width + height);
    let mut points = Vec::with_capacity((length / 10.0) as usize + 4);
    let mut traveled = 0.0;

    for ((start_x, start_y), (dx, dy), side) in [
        ((0.0, 0.0), (1.0, 0.0), width),
        ((width, 0.0), (0.0, 1.0), height),
        ((width, height), (-1.0, 0.0), width),
        ((0.0, height), (0.0, -1.0), height),
    ] {
        let (nx, ny) = (dy, -dx);
        points.push(EdgePoint {
            x: start_x,
            y: start_y,
            nx: 0.0,
            ny: 0.0,
            phase: TAU * traveled / length,
            ripple_weight: 0.0,
        });
        let steps = (side / 10.0).ceil() as usize;
        for step in 1..steps {
            let offset = step as f32 * side / steps as f32;
            let corner_fade = (offset.min(side - offset) / 28.0).min(1.0);
            points.push(EdgePoint {
                x: start_x + dx * offset,
                y: start_y + dy * offset,
                nx,
                ny,
                phase: TAU * (traveled + offset) / length,
                ripple_weight: corner_fade * corner_fade * (3.0 - 2.0 * corner_fade),
            });
        }
        traveled += side;
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_moves_the_ripple_inward_and_leaves_the_corners_flush() {
        let mut border = RippleBorder::default();
        assert!(border.update(820.0, 600.0, 0.0, 0.016).is_none());
        let active = border.update(820.0, 600.0, 0.8, 0.016).unwrap();
        let next = border.update(820.0, 600.0, 0.8, 0.016).unwrap();
        assert!(active.1 > 0.0);
        assert_ne!(active.0, next.0);
        for corner in ["M0.0 0.0", "L820.0 0.0", "L820.0 600.0", "L0.0 600.0"] {
            assert!(active.0.contains(corner));
            assert!(next.0.contains(corner));
        }

        let mut settled = next;
        for _ in 0..40 {
            if let Some(frame) = border.update(820.0, 600.0, 0.0, 0.016) {
                settled = frame;
            }
        }
        assert_eq!(settled, (String::new(), 0.0));
        assert!(border.update(900.0, 600.0, 0.0, 0.016).is_none());
    }
}
