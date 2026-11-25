//! A text editor covers one widget rather than allocating an output-sized surface.

/// Enough space for the existing text field and its header and actions.
pub const MIN_WIDTH: f32 = 240.0;
/// Single-line task editor, including its header and actions.
pub const TASK_HEIGHT: f32 = 144.0;
/// Multiline notes retain a useful writing area even on short notes.
pub const NOTE_HEIGHT: f32 = 280.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub output: String,
    pub bounds: [f32; 4],
    pub output_size: (f32, f32),
}

impl Placement {
    /// Use the compositor's logical output geometry. Clamp to that output
    /// without assuming anything about its global position or scale.
    pub fn editor_bounds(&self, multiline: bool) -> Option<[f32; 4]> {
        let (width, height) = self.output_size;
        if self.output.is_empty()
            || !self.bounds.iter().chain([&width, &height]).all(|v| v.is_finite())
            || width < 1.0
            || height < 1.0
            || self.bounds[2] <= 0.0
            || self.bounds[3] <= 0.0
        {
            return None;
        }
        let w = self.bounds[2].max(MIN_WIDTH).ceil().min(width.floor());
        let h = self.bounds[3].max(if multiline { NOTE_HEIGHT } else { TASK_HEIGHT }).ceil().min(height.floor());
        let x = self.bounds[0].clamp(0.0, (width - w).max(0.0)).floor();
        let y = self.bounds[1].clamp(0.0, (height - h).max(0.0)).floor();
        Some([x, y, w, h])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_widget_bounds_stay_on_the_clicked_output() {
        let p = Placement { output: "DP-2".into(), bounds: [800.25, 700.75, 320.5, 210.25], output_size: (1000.0, 800.0) };
        assert_eq!(p.editor_bounds(true), Some([679.0, 520.0, 321.0, NOTE_HEIGHT]));
        assert_eq!(p.output, "DP-2");
        let task = Placement { bounds: [10.5, 20.5, 310.0, 90.0], ..p };
        assert_eq!(task.editor_bounds(false), Some([10.0, 20.0, 310.0, TASK_HEIGHT]));
    }

    #[test]
    fn tiny_or_invalid_outputs_never_produce_zero_sized_surfaces() {
        let mut p = Placement { output: "small".into(), bounds: [-10.0, -10.0, 100.0, 100.0], output_size: (120.0, 90.0) };
        assert_eq!(p.editor_bounds(true), Some([0.0, 0.0, 120.0, 90.0]));
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            p.output_size.0 = invalid;
            assert_eq!(p.editor_bounds(true), None);
        }
    }
}
