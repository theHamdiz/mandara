//! Quiet clock faces: word typography, mechanical cards, glass tubes and BCD.

use mandara_config::desktop_widgets::{ClockFace, ClockWidget, FontRole};
use mandara_i18n::{fill, tk};

use super::layout::{ClockStrings, InkRole, Mark, MarkKind, Measure, face, style, wrap};
use super::lingo::Lingo;

/// Reference geometry at 1080 px, scaled with the widget's placement.
const DIGIT_W: f32 = 84.0;
const DIGIT_H: f32 = 120.0;
const DIGIT_SIZE: f32 = 88.0;
const DIGIT_GAP: f32 = 10.0;
const PAIR_GAP: f32 = 24.0;
const RADIUS: f32 = 12.0;
const STROKE: f32 = 1.5;
const CAPTION: f32 = 18.0;
const CAPTION_GAP: f32 = 16.0;
const WORD_SIZE: f32 = 56.0;
const WORD_WIDTH: f32 = 540.0;
const WORD_LINES: usize = 4;
const BIT_PITCH: f32 = 36.0;
const BIT_R: f32 = 11.0;
const MUTED: f32 = 0.18;
const SOFT: f32 = 0.6;
/// Four BCD bits per decimal digit, in the order painted top to bottom.
const BIT_WEIGHTS: [u32; 4] = [8, 4, 2, 1];
const HOURS: [&str; 12] = [tk("Twelve"), tk("One"), tk("Two"), tk("Three"), tk("Four"), tk("Five"), tk("Six"), tk("Seven"), tk("Eight"), tk("Nine"), tk("Ten"), tk("Eleven")];
const PHRASES: [&str; 12] = [tk("{hour} o'clock"), tk("Five past {hour}"), tk("Ten past {hour}"), tk("Quarter past {hour}"), tk("Twenty past {hour}"), tk("Twenty-five past {hour}"), tk("Half past {hour}"), tk("Twenty-five to {hour}"), tk("Twenty to {hour}"), tk("Quarter to {hour}"), tk("Ten to {hour}"), tk("Five to {hour}")];

/// The nearest five-minute phrase; the face also prints the exact time.
fn phrase(hour: u32, minute: u32, l: &Lingo) -> String {
    let step = (minute.min(59) + 2) / 5;
    let next = u32::from(step > 6);
    let hour = l.word(HOURS[((hour + next) % 12) as usize]);
    fill(&l.word(PHRASES[(step % 12) as usize]), &[("hour", &hour)])
}

fn text(marks: &mut Vec<Mark>, value: &str, role: FontRole, size: f32, center: f32, y: f32, ink: InkRole, m: &mut dyn Measure) -> f32 {
    let font = style(role, size, face::MEDIUM);
    let (w, h) = m.measure(value, &font);
    marks.push(Mark::new(MarkKind::Text { text: value.into(), style: font, x: center - w / 2.0, y, w, h }, ink));
    h
}

fn digits(s: &ClockStrings) -> [u32; 4] {
    // Respect the selected 12/24-hour display while keeping BCD independent
    // of the glyphs used for localized numerals.
    let hour = if s.meridiem.is_some() { match s.hour % 12 { 0 => 12, h => h } } else { s.hour % 24 };
    [hour / 10, hour % 10, s.minute / 10, s.minute % 10]
}

pub fn clock(c: &ClockWidget, s: &ClockStrings, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let probe = Lingo::of(&c.locale, s.rtl, None);
    let l = Lingo { eastern: c.eastern(probe.arabic()), ..probe };
    let mut marks = Vec::new();
    if c.face == ClockFace::Word {
        let font = style(c.look.font, WORD_SIZE * k, face::SEMIBOLD);
        let words = phrase(s.hour, s.minute, &l);
        let lines = wrap(&words, &font, WORD_WIDTH * k, WORD_LINES, m);
        let mut y = 0.0;
        for value in lines {
            y += text(&mut marks, &value, c.look.font, WORD_SIZE * k, WORD_WIDTH * k / 2.0, y, InkRole::Fg, m);
        }
        text(&mut marks, &s.time, FontRole::Mono, CAPTION * k, WORD_WIDTH * k / 2.0, y + CAPTION_GAP * k, InkRole::Accent, m);
        return marks;
    }
    let width = (DIGIT_W * 4.0 + DIGIT_GAP * 2.0 + PAIR_GAP) * k;
    for (i, digit) in digits(s).into_iter().enumerate() {
        let x = (i as f32 * (DIGIT_W + DIGIT_GAP) + if i >= 2 { PAIR_GAP - DIGIT_GAP } else { 0.0 }) * k;
        let center = x + DIGIT_W * k / 2.0;
        let value = l.digits(digit.to_string());
        if c.face == ClockFace::Binary {
            for (row, weight) in BIT_WEIGHTS.into_iter().enumerate() {
                let on = digit & weight != 0;
                marks.push(Mark::new(MarkKind::Disc { c: (center, (row as f32 + 0.5) * BIT_PITCH * k), r: BIT_R * k }, if on { InkRole::Accent } else { InkRole::Fg }).dim(if on { 1.0 } else { MUTED }));
            }
            text(&mut marks, &value, FontRole::Mono, CAPTION * k, center, BIT_PITCH * BIT_WEIGHTS.len() as f32 * k, InkRole::Fg, m);
        } else {
            let tube = c.face == ClockFace::Nixie;
            marks.push(Mark::new(MarkKind::Rect { x, y: 0.0, w: DIGIT_W * k, h: DIGIT_H * k, radius: if tube { DIGIT_W / 2.0 } else { RADIUS } * k, width: if tube { STROKE * k } else { 0.0 } }, if tube { InkRole::Accent } else { InkRole::Plate }).flat());
            let font = style(FontRole::Mono, DIGIT_SIZE * k, face::MEDIUM);
            let (_, h) = m.measure(&value, &font);
            text(&mut marks, &value, FontRole::Mono, DIGIT_SIZE * k, center, (DIGIT_H * k - h) / 2.0, if tube { InkRole::Accent } else { InkRole::OnPlate }, m);
            if tube {
                marks.push(Mark::new(MarkKind::Line { a: (x + RADIUS * k, DIGIT_H * k + STROKE * k), b: (x + (DIGIT_W - RADIUS) * k, DIGIT_H * k + STROKE * k), width: STROKE * k }, InkRole::Fg).dim(SOFT));
            } else {
                marks.push(Mark::new(MarkKind::Line { a: (x, DIGIT_H * k / 2.0), b: (x + DIGIT_W * k, DIGIT_H * k / 2.0), width: STROKE * k }, InkRole::Fg).dim(MUTED).flat());
            }
        }
    }
    let y = if c.face == ClockFace::Binary { (BIT_PITCH * BIT_WEIGHTS.len() as f32 + CAPTION + CAPTION_GAP) * k } else { (DIGIT_H + CAPTION_GAP) * k };
    text(&mut marks, &format!("{} · {}", s.weekday_short, s.day_month), c.look.font, CAPTION * k, width / 2.0, y, InkRole::Fg, m);
    marks
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::layout::tests::{Fixed, strings};

    #[test]
    fn phrases_roll_over_at_the_hour_and_midnight() {
        let l = Lingo::of(&mandara_config::desktop_widgets::TextOptions { language: "en".into(), ..Default::default() }, false, None);
        assert_eq!(phrase(9, 15, &l), "Quarter past Nine");
        assert_eq!(phrase(23, 59, &l), "Twelve o'clock");
        assert_eq!(phrase(12, 30, &l), "Half past Twelve");
    }

    #[test]
    fn binary_columns_encode_the_displayed_time() {
        let mut s = strings();
        s.hour = 23; s.minute = 59; s.meridiem = None;
        assert_eq!(digits(&s), [2, 3, 5, 9]);
        s.meridiem = Some("PM".into());
        assert_eq!(digits(&s), [1, 1, 5, 9]);
        let marks = clock(&ClockWidget { face: ClockFace::Binary, ..Default::default() }, &s, 1.0, &mut Fixed);
        assert_eq!(marks.iter().filter(|m| matches!(m.kind, MarkKind::Disc { .. }) && m.opacity == 1.0).count(), 6);
    }

    #[test]
    fn all_faces_have_finite_ink_at_every_scale() {
        for face in [ClockFace::Word, ClockFace::SplitFlap, ClockFace::Nixie, ClockFace::Binary] {
            for k in [0.5, 1.0, 2.0] {
                let marks = clock(&ClockWidget { face, ..Default::default() }, &strings(), k, &mut Fixed);
                assert!(!marks.is_empty());
                assert!(marks.iter().all(|m| { let b = m.bounds(); [b.0, b.1, b.2, b.3].into_iter().all(f32::is_finite) }));
            }
        }
    }
}
