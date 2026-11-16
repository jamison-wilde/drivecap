//! Card geometry and hit testing (handoff §Card / §Header / §Row). Pure:
//! takes a [`SurfaceView`], DPI metrics and a text measurer, returns
//! rectangles in physical pixels relative to the layered window's origin.
//! The window is the card plus [`HALO_MARGIN`] of transparent space on
//! every side for the dire halo.
//!
//! Vertical offsets inside a row are scaled cumulatively (`px(5)`,
//! `px(25)`, …) rather than piece by piece, so rounding at fractional
//! scales never drifts the bar off the row's bottom padding.

use crate::model::SurfaceView;

/// Integer rectangle; [`Rect::contains`] is half-open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// `x ∈ [x, x+w)` and `y ∈ [y, y+h)`; empty rects contain nothing.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// Font role for measuring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextStyle {
    Body,
    BodySemibold,
    BodyBold,
    Small,
    Glyph,
}

/// Text width in physical pixels for the card's font at the current DPI.
pub trait TextMeasure {
    fn width(&self, text: &str, style: TextStyle) -> i32;
}

/// Combined DPI × config scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub s: f32,
}

impl Metrics {
    /// `dpi / 96 × scale_pct / 100`.
    pub fn new(dpi: u32, scale_pct: u32) -> Metrics {
        Metrics {
            s: dpi as f32 / 96.0 * scale_pct as f32 / 100.0,
        }
    }

    /// Logical (100 %, 96 DPI) px → physical px, rounded.
    pub fn px(&self, logical: i32) -> i32 {
        (logical as f32 * self.s).round() as i32
    }
}

/// Header icon buttons, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderButton {
    SnoozeAll,
    Opacity,
    Settings,
    Chevron,
}

/// What a point in the window lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Grip,
    /// Header area that is not a button (title included): drags.
    HeaderEmpty,
    Button(HeaderButton),
    RowSnooze(usize),
    Row(usize),
    /// Halo margin or outside the window.
    Outside,
}

/// Every rectangle the painter and hit test need.
#[derive(Clone, Debug, PartialEq)]
pub struct CardLayout {
    /// Card plus halo margin; `(0, 0)` is the window origin.
    pub window: Rect,
    pub card: Rect,
    pub header: Rect,
    pub grip: Rect,
    pub dot: Rect,
    /// Title text box (full header height; width clipped to the space left).
    pub title: Rect,
    pub buttons: [(HeaderButton, Rect); 4],
    pub rows: Vec<RowLayout>,
}

/// One row's rectangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowLayout {
    /// Whole row including its 1 px top border.
    pub row: Rect,
    pub letter: Rect,
    pub label: Rect,
    pub value: Rect,
    /// Zz button; zero width on rows without one (the column stays reserved).
    pub snooze: Rect,
    pub bar: Rect,
}

/// Transparent margin around the card for the dire halo (logical px).
pub const HALO_MARGIN: i32 = 8;

const CARD_W: i32 = 270;
const HEADER_H: i32 = 28;
const HEADER_PAD_L: i32 = 9;
const HEADER_PAD_R: i32 = 3;
const GRIP_W: i32 = 6;
const GRIP_H: i32 = 12;
const DOT: i32 = 8;
const HEADER_GAP: i32 = 6;
const BUTTON: i32 = 22;
const BUTTON_GAP: i32 = 2;
/// 1 border + 4 top + 20 line 1 + 3 gap + 3 bar + 8 bottom.
const ROW_H: i32 = 39;
const LINE1_TOP: i32 = 5;
const LINE1_BOTTOM: i32 = 25;
const BAR_TOP: i32 = 28;
const BAR_BOTTOM: i32 = 31;
const ROW_PAD_L: i32 = 10;
const ROW_PAD_R: i32 = 6;
const LETTER_W: i32 = 24;
const COL_GAP: i32 = 6;
const SNOOZE_W: i32 = 20;
const BAR_INSET_R: i32 = 4;
const OPACITY_POPUP_W: i32 = 200;
/// 8 pad + 16 label line + 4 gap + 20 slider + 8 pad.
const OPACITY_POPUP_H: i32 = 56;
const SETTINGS_POPUP_W: i32 = 230;

/// Lays out the card for `view`. Minimized with no rows → header only.
pub fn compute(view: &SurfaceView, m: &Metrics, tm: &dyn TextMeasure) -> CardLayout {
    let margin = m.px(HALO_MARGIN);
    let row_h = m.px(ROW_H);
    let header_h = m.px(HEADER_H);
    let n = view.rows.len() as i32;
    let card = Rect::new(margin, margin, m.px(CARD_W), header_h + n * row_h);
    let window = Rect::new(0, 0, card.w + 2 * margin, card.h + 2 * margin);
    let header = Rect::new(card.x, card.y, card.w, header_h);
    let centred = |w: i32, h: i32, x: i32| Rect::new(x, header.y + (header.h - h) / 2, w, h);

    let grip = centred(m.px(GRIP_W), m.px(GRIP_H), card.x + m.px(HEADER_PAD_L));
    let dot = centred(m.px(DOT), m.px(DOT), grip.right() + m.px(HEADER_GAP));

    let order = [
        HeaderButton::SnoozeAll,
        HeaderButton::Opacity,
        HeaderButton::Settings,
        HeaderButton::Chevron,
    ];
    let (bw, gap) = (m.px(BUTTON), m.px(BUTTON_GAP));
    let last_right = card.right() - m.px(HEADER_PAD_R);
    let buttons = order.map(|b| {
        let from_right = 3 - b as i32;
        let x = last_right - bw - from_right * (bw + gap);
        (b, centred(bw, bw, x))
    });

    let title_x = dot.right() + m.px(HEADER_GAP);
    let title_room = (buttons[0].1.x - title_x).max(0);
    let title_w = tm
        .width(&view.title, TextStyle::BodySemibold)
        .min(title_room);
    let title = Rect::new(title_x, header.y, title_w.max(0), header.h);

    let value_w = view
        .rows
        .iter()
        .map(|r| tm.width(&r.value, TextStyle::BodySemibold))
        .max()
        .unwrap_or(0);
    let rows = view
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let row = Rect::new(card.x, header.bottom() + i as i32 * row_h, card.w, row_h);
            row_layout(row, m, value_w, r.show_snooze_btn)
        })
        .collect();

    CardLayout {
        window,
        card,
        header,
        grip,
        dot,
        title,
        buttons,
        rows,
    }
}

/// Rectangles inside one row. `value_w` is the shared value-column width.
fn row_layout(row: Rect, m: &Metrics, value_w: i32, has_snooze: bool) -> RowLayout {
    let line_y = row.y + m.px(LINE1_TOP);
    let line_h = m.px(LINE1_BOTTOM) - m.px(LINE1_TOP);
    let left = row.x + m.px(ROW_PAD_L);
    let right = row.right() - m.px(ROW_PAD_R);

    let letter = Rect::new(left, line_y, m.px(LETTER_W), line_h);
    let snooze_x = right - m.px(SNOOZE_W);
    let snooze_w = if has_snooze { m.px(SNOOZE_W) } else { 0 };
    let snooze = Rect::new(snooze_x, line_y, snooze_w, line_h);
    let value_right = snooze_x - m.px(COL_GAP);
    let value = Rect::new(value_right - value_w, line_y, value_w, line_h);
    let label_x = letter.right() + m.px(COL_GAP);
    let label_w = (value.x - m.px(COL_GAP) - label_x).max(0);
    let label = Rect::new(label_x, line_y, label_w, line_h);
    let bar = Rect::new(
        left,
        row.y + m.px(BAR_TOP),
        right - m.px(BAR_INSET_R) - left,
        m.px(BAR_BOTTOM) - m.px(BAR_TOP),
    );
    RowLayout {
        row,
        letter,
        label,
        value,
        snooze,
        bar,
    }
}

/// What `(x, y)` (window coordinates) lands on.
pub fn hit_test(l: &CardLayout, x: i32, y: i32) -> Hit {
    if !l.card.contains(x, y) {
        return Hit::Outside;
    }
    if let Some((b, _)) = l.buttons.iter().find(|(_, r)| r.contains(x, y)) {
        return Hit::Button(*b);
    }
    if l.grip.contains(x, y) {
        return Hit::Grip;
    }
    if l.header.contains(x, y) {
        return Hit::HeaderEmpty;
    }
    for (i, r) in l.rows.iter().enumerate() {
        if r.snooze.contains(x, y) {
            return Hit::RowSnooze(i);
        }
        if r.row.contains(x, y) {
            return Hit::Row(i);
        }
    }
    Hit::Outside
}

/// Opacity popup: 200 wide, directly under the header, right edge on the
/// Opacity button's right edge. Window coordinates; the caller offsets
/// by the window's screen position.
pub fn opacity_popup_rect(l: &CardLayout, m: &Metrics) -> Rect {
    popup_under(
        l,
        HeaderButton::Opacity,
        m.px(OPACITY_POPUP_W),
        m.px(OPACITY_POPUP_H),
    )
}

/// Settings popup: 230 wide, `content_h` (physical px) tall, under the
/// header, right-aligned to the Settings button.
pub fn settings_popup_rect(l: &CardLayout, m: &Metrics, content_h: i32) -> Rect {
    popup_under(l, HeaderButton::Settings, m.px(SETTINGS_POPUP_W), content_h)
}

fn popup_under(l: &CardLayout, which: HeaderButton, w: i32, h: i32) -> Rect {
    let btn = l.buttons[which as usize].1;
    Rect::new(btn.right() - w, l.header.bottom(), w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, VolumeId};
    use crate::model::{Dot, RowState, RowView};

    /// 7 px per char, independent of style.
    struct Fixed;
    impl TextMeasure for Fixed {
        fn width(&self, text: &str, _style: TextStyle) -> i32 {
            7 * text.chars().count() as i32
        }
    }

    fn row(value: &str, btn: bool) -> RowView {
        RowView {
            id: VolumeId("v".into()),
            letter: "C:".into(),
            label: "Windows".into(),
            value: value.into(),
            state: if btn {
                RowState::Warning
            } else {
                RowState::Healthy
            },
            fill: 0.5,
            gaps: vec![],
            show_snooze_btn: btn,
            snooze_active: false,
        }
    }

    fn view(rows: Vec<RowView>) -> SurfaceView {
        SurfaceView {
            title: "Disk space".into(),
            dot: Dot::Warning,
            snooze_all_active: false,
            rows,
            halo: false,
            mode: Mode::Expanded,
        }
    }

    fn three_rows() -> SurfaceView {
        view(vec![
            row("1.2 GB", true),
            row("14.8 GB", true),
            row("245 GB", false),
        ])
    }

    fn lay(v: &SurfaceView, pct: u32) -> CardLayout {
        compute(v, &Metrics::new(96, pct), &Fixed)
    }

    fn btn(l: &CardLayout, b: HeaderButton) -> Rect {
        l.buttons.iter().find(|(k, _)| *k == b).unwrap().1
    }

    #[test]
    fn metrics_combine_dpi_and_scale() {
        assert_eq!(Metrics::new(96, 100).px(270), 270);
        assert_eq!(Metrics::new(144, 100).px(270), 405);
        assert_eq!(Metrics::new(96, 150).px(270), 405);
        assert_eq!(Metrics::new(120, 100).px(3), 4); // 3.75 rounds up
    }

    #[test]
    fn card_width_270_at_100pct() {
        assert_eq!(lay(&three_rows(), 100).card.w, 270);
    }

    #[test]
    fn card_width_scales_150pct() {
        assert_eq!(lay(&three_rows(), 150).card.w, 405);
    }

    #[test]
    fn window_has_halo_margin() {
        for pct in [100, 150] {
            let l = lay(&three_rows(), pct);
            let m = Metrics::new(96, pct).px(HALO_MARGIN);
            assert_eq!((l.card.x, l.card.y), (m, m));
            assert_eq!(
                l.window,
                Rect::new(0, 0, l.card.w + 2 * m, l.card.h + 2 * m)
            );
        }
    }

    #[test]
    fn height_header_plus_rows() {
        assert_eq!(lay(&view(vec![]), 100).card.h, 28);
        assert!(lay(&view(vec![]), 100).rows.is_empty());
        assert_eq!(lay(&three_rows(), 100).card.h, 28 + 3 * 39);
    }

    #[test]
    fn row_internals_at_100pct() {
        let l = lay(&three_rows(), 100);
        let r = l.rows[1];
        assert_eq!(r.row, Rect::new(8, 8 + 28 + 39, 270, 39));
        assert_eq!(r.letter, Rect::new(18, r.row.y + 5, 24, 20));
        assert_eq!(r.label.x, 18 + 24 + 6);
        assert_eq!(r.snooze, Rect::new(8 + 270 - 6 - 20, r.row.y + 5, 20, 20));
        assert_eq!(r.bar, Rect::new(18, r.row.y + 28, 270 - 10 - 6 - 4, 3));
        assert_eq!(r.bar.bottom() + 8, r.row.bottom());
    }

    #[test]
    fn header_left_items() {
        let l = lay(&three_rows(), 100);
        assert_eq!(l.header, Rect::new(8, 8, 270, 28));
        assert_eq!(l.grip, Rect::new(8 + 9, 8 + 8, 6, 12));
        assert_eq!(l.dot, Rect::new(l.grip.right() + 6, 8 + 10, 8, 8));
        assert_eq!(l.title.x, l.dot.right() + 6);
        assert_eq!(l.title.w, 7 * "Disk space".len() as i32);
    }

    #[test]
    fn buttons_right_aligned() {
        let l = lay(&three_rows(), 100);
        let order: Vec<HeaderButton> = l.buttons.iter().map(|(b, _)| *b).collect();
        assert_eq!(
            order,
            [
                HeaderButton::SnoozeAll,
                HeaderButton::Opacity,
                HeaderButton::Settings,
                HeaderButton::Chevron
            ]
        );
        assert_eq!(btn(&l, HeaderButton::Chevron).right(), l.card.right() - 3);
        for pair in l.buttons.windows(2) {
            assert_eq!(pair[1].1.x - pair[0].1.right(), 2);
        }
        for (_, r) in &l.buttons {
            assert_eq!((r.w, r.h, r.y), (22, 22, l.header.y + 3));
        }
    }

    #[test]
    fn value_column_aligned_across_rows() {
        let l = lay(&three_rows(), 100);
        let widest = 7 * "14.8 GB".len() as i32;
        let right = l.rows[0].value.right();
        for r in &l.rows {
            assert_eq!(r.value.right(), right);
            assert_eq!(r.value.w, widest);
            assert_eq!(
                r.value.right() + 6,
                r.snooze.x,
                "column reserved without a button"
            );
            assert_eq!(r.label.right() + 6, r.value.x);
        }
        assert_eq!(l.rows[2].snooze.w, 0);
    }

    #[test]
    fn hit_test_regions() {
        let l = lay(&three_rows(), 100);
        let mid = |r: Rect| (r.x + r.w / 2, r.y + r.h / 2);
        let (gx, gy) = mid(l.grip);
        assert_eq!(hit_test(&l, gx, gy), Hit::Grip);
        for (b, r) in &l.buttons {
            let (x, y) = mid(*r);
            assert_eq!(hit_test(&l, x, y), Hit::Button(*b));
        }
        let (tx, ty) = mid(l.title);
        assert_eq!(hit_test(&l, tx, ty), Hit::HeaderEmpty);
        let (sx, sy) = mid(l.rows[1].snooze);
        assert_eq!(hit_test(&l, sx, sy), Hit::RowSnooze(1));
        let (lx, ly) = mid(l.rows[1].label);
        assert_eq!(hit_test(&l, lx, ly), Hit::Row(1));
        // Reserved snooze column on a button-less row is plain row body.
        let (rx, ry) = (l.rows[2].snooze.x + 10, l.rows[2].snooze.y + 10);
        assert_eq!(hit_test(&l, rx, ry), Hit::Row(2));
        assert_eq!(hit_test(&l, 3, 3), Hit::Outside);
        assert_eq!(hit_test(&l, l.card.right(), l.card.y + 5), Hit::Outside);
        assert_eq!(hit_test(&l, l.card.x + 5, l.card.bottom()), Hit::Outside);
    }

    #[test]
    fn opacity_popup_right_aligned_to_button() {
        let l = lay(&three_rows(), 100);
        let p = opacity_popup_rect(&l, &Metrics::new(96, 100));
        assert_eq!(p.right(), btn(&l, HeaderButton::Opacity).right());
        assert_eq!((p.y, p.w), (l.header.bottom(), 200));
    }

    #[test]
    fn settings_popup_right_aligned_to_button() {
        let l = lay(&three_rows(), 150);
        let p = settings_popup_rect(&l, &Metrics::new(96, 150), 300);
        assert_eq!(p.right(), btn(&l, HeaderButton::Settings).right());
        assert_eq!((p.y, p.w, p.h), (l.header.bottom(), 345, 300));
    }

    #[test]
    fn long_title_clipped_before_buttons() {
        let mut v = three_rows();
        v.title = "x".repeat(200);
        let l = lay(&v, 100);
        assert_eq!(l.title.right(), btn(&l, HeaderButton::SnoozeAll).x);
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(10, 10, 5, 5);
        assert!(r.contains(10, 10) && r.contains(14, 14));
        assert!(!r.contains(15, 10) && !r.contains(10, 15) && !r.contains(9, 10));
        assert!(!Rect::new(0, 0, 0, 10).contains(0, 0));
    }
}
