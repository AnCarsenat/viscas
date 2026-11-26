//! One API over the two kinds of math input: a multi-line
//! [`gtk::TextView`] (console) and a single-line [`gtk::Entry`]
//! (expression rows, inspector fields). Offsets are chars.

use gtk::prelude::*;
use gtk::{gdk, graphene, pango};
use viscas_engine::Severity;

use crate::theme::model::Color;

/// A math input widget.
#[derive(Clone)]
pub enum Field {
    View(gtk::TextView),
    Entry(gtk::Entry),
}

const ERROR_TAG: &str = "assist-error";
const WARNING_TAG: &str = "assist-warning";

impl Field {
    pub fn widget(&self) -> gtk::Widget {
        match self {
            Field::View(v) => v.clone().upcast(),
            Field::Entry(e) => e.clone().upcast(),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Field::View(v) => {
                let b = v.buffer();
                b.text(&b.start_iter(), &b.end_iter(), false).to_string()
            }
            Field::Entry(e) => e.text().to_string(),
        }
    }

    /// Caret position.
    pub fn cursor(&self) -> usize {
        match self {
            Field::View(v) => v.buffer().cursor_position().max(0) as usize,
            Field::Entry(e) => e.position().max(0) as usize,
        }
    }

    /// Selected range, if any (ordered).
    pub fn selection(&self) -> Option<(usize, usize)> {
        let (s, e) = match self {
            Field::View(v) => {
                let (a, b) = v.buffer().selection_bounds()?;
                (a.offset(), b.offset())
            }
            Field::Entry(e) => e.selection_bounds()?,
        };
        let (s, e) = (s.max(0) as usize, e.max(0) as usize);
        (s != e).then_some((s.min(e), s.max(e)))
    }

    pub fn set_cursor(&self, pos: usize) {
        match self {
            Field::View(v) => {
                let b = v.buffer();
                b.place_cursor(&b.iter_at_offset(pos as i32));
            }
            Field::Entry(e) => e.set_position(pos as i32),
        }
    }

    /// Selects `start..end`, the caret at `end`.
    pub fn select(&self, start: usize, end: usize) {
        match self {
            Field::View(v) => {
                let b = v.buffer();
                b.select_range(&b.iter_at_offset(end as i32), &b.iter_at_offset(start as i32));
            }
            Field::Entry(e) => e.select_region(start as i32, end as i32),
        }
    }

    /// Replaces `start..end` with `text` as one user edit (undoable like
    /// typing).
    pub fn replace(&self, start: usize, end: usize, text: &str) {
        match self {
            Field::View(v) => {
                let b = v.buffer();
                b.begin_user_action();
                let mut s = b.iter_at_offset(start as i32);
                let mut e = b.iter_at_offset(end as i32);
                if start != end {
                    b.delete(&mut s, &mut e);
                }
                if !text.is_empty() {
                    let mut at = b.iter_at_offset(start as i32);
                    b.insert(&mut at, text);
                }
                b.end_user_action();
            }
            Field::Entry(en) => {
                if start != end {
                    en.delete_text(start as i32, end as i32);
                }
                if !text.is_empty() {
                    let mut pos = start as i32;
                    en.insert_text(text, &mut pos);
                }
            }
        }
    }

    /// Whether the widget (or its inner text) has the keyboard focus.
    pub fn has_focus(&self) -> bool {
        self.widget()
            .state_flags()
            .contains(gtk::StateFlags::FOCUS_WITHIN)
    }

    /// Rectangle of the caret at char `pos`, in `target` coordinates.
    pub fn caret_rect(&self, pos: usize, target: &gtk::Widget) -> Option<gdk::Rectangle> {
        match self {
            Field::View(v) => {
                let it = v.buffer().iter_at_offset(pos as i32);
                let r = v.iter_location(&it);
                let (x, y) = v.buffer_to_window_coords(gtk::TextWindowType::Widget, r.x(), r.y());
                let p = v.compute_point(target, &graphene::Point::new(x as f32, y as f32))?;
                Some(gdk::Rectangle::new(
                    p.x() as i32,
                    p.y() as i32,
                    1,
                    r.height().max(1),
                ))
            }
            Field::Entry(e) => {
                let text = inner_text(e)?;
                let (strong, _) = text.compute_cursor_extents(pos);
                let p = text.compute_point(target, &strong.top_left())?;
                Some(gdk::Rectangle::new(
                    p.x() as i32,
                    p.y() as i32,
                    1,
                    (strong.height() as i32).max(1),
                ))
            }
        }
    }

    /// The char under point (`x`, `y`) of the field widget.
    pub fn char_at(&self, x: f64, y: f64) -> Option<usize> {
        match self {
            Field::View(v) => {
                let (bx, by) =
                    v.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
                let (it, _) = v.iter_at_position(bx, by)?;
                Some(it.offset().max(0) as usize)
            }
            Field::Entry(e) => {
                let text = inner_text(e)?;
                let p = e.compute_point(&text, &graphene::Point::new(x as f32, y as f32))?;
                let n = e.text().chars().count();
                let mut hit = None;
                for i in 0..n {
                    let (a, _) = text.compute_cursor_extents(i);
                    let (b, _) = text.compute_cursor_extents(i + 1);
                    let (l, r) = (a.x().min(b.x()), a.x().max(b.x()));
                    if p.x() >= l && p.x() < r {
                        hit = Some(i);
                        break;
                    }
                }
                hit
            }
        }
    }

    /// Underlines byte ranges of the text: diagnostics (squiggles in the
    /// error / warning colors) and `extra` ranges (committed errors the
    /// editor reports) in the error color.
    pub fn set_marks(
        &self,
        marks: &[(usize, usize, Severity)],
        extra: &[(usize, usize)],
        error: Color,
        warning: Color,
    ) {
        let text = self.text();
        let all = marks
            .iter()
            .copied()
            .chain(extra.iter().map(|(s, e)| (*s, *e, Severity::Error)))
            .filter(|(s, e, _)| {
                s < e && *e <= text.len() && text.is_char_boundary(*s) && text.is_char_boundary(*e)
            });
        match self {
            Field::View(v) => {
                let b = v.buffer();
                let table = b.tag_table();
                let tag = |name: &str, c: Color| {
                    let t = table.lookup(name).unwrap_or_else(|| {
                        let t = gtk::TextTag::new(Some(name));
                        t.set_underline(pango::Underline::Error);
                        table.add(&t);
                        t
                    });
                    t.set_underline_rgba(Some(&rgba(c)));
                    t
                };
                let err = tag(ERROR_TAG, error);
                let warn = tag(WARNING_TAG, warning);
                b.remove_tag(&err, &b.start_iter(), &b.end_iter());
                b.remove_tag(&warn, &b.start_iter(), &b.end_iter());
                for (s, e, sev) in all {
                    let s = b.iter_at_offset(text[..s].chars().count() as i32);
                    let e = b.iter_at_offset(text[..e].chars().count() as i32);
                    b.apply_tag(if sev == Severity::Error { &err } else { &warn }, &s, &e);
                }
            }
            Field::Entry(en) => {
                let attrs = pango::AttrList::new();
                for (s, e, sev) in all {
                    let mut u = pango::AttrInt::new_underline(pango::Underline::Error);
                    u.set_start_index(s as u32);
                    u.set_end_index(e as u32);
                    attrs.insert(u);
                    let c = if sev == Severity::Error { error } else { warning };
                    let scale = |v: u8| u16::from(v) * 257;
                    let mut uc =
                        pango::AttrColor::new_underline_color(scale(c.r), scale(c.g), scale(c.b));
                    uc.set_start_index(s as u32);
                    uc.set_end_index(e as u32);
                    attrs.insert(uc);
                }
                en.set_attributes(&attrs);
            }
        }
    }

    /// Calls `f` after every text change.
    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        match self {
            Field::View(v) => {
                v.buffer().connect_changed(move |_| f());
            }
            Field::Entry(e) => {
                e.connect_changed(move |_| f());
            }
        }
    }

    /// Calls `f` when the caret moves.
    pub fn connect_cursor_moved(&self, f: impl Fn() + 'static) {
        match self {
            Field::View(v) => {
                v.buffer()
                    .connect_cursor_position_notify(move |_| f());
            }
            Field::Entry(e) => {
                e.connect_cursor_position_notify(move |_| f());
            }
        }
    }
}

fn inner_text(e: &gtk::Entry) -> Option<gtk::Text> {
    e.delegate()?.downcast::<gtk::Text>().ok()
}

fn rgba(c: Color) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(c.r) / 255.0,
        f32::from(c.g) / 255.0,
        f32::from(c.b) / 255.0,
        1.0,
    )
}
