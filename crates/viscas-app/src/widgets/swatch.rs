//! A color swatch button; clicking opens GTK's color picker.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};

use crate::theme::model::Color;

type Listener = Rc<dyn Fn(Color)>;

struct Inner {
    button: gtk::Button,
    area: gtk::DrawingArea,
    color: Cell<Color>,
    title: String,
    listeners: RefCell<Vec<Listener>>,
}

/// Color swatch. Cloning yields a handle to the same widget.
#[derive(Clone)]
pub struct ColorSwatch {
    inner: Rc<Inner>,
}

impl ColorSwatch {
    pub fn new(color: Color, title: &str) -> Self {
        let area = gtk::DrawingArea::builder()
            .hexpand(true)
            .content_height(14)
            .can_target(false)
            .build();
        let button = gtk::Button::builder()
            .child(&area)
            .hexpand(true)
            .css_classes(["swatch"])
            .build();
        let swatch = Self {
            inner: Rc::new(Inner {
                button,
                area,
                color: Cell::new(color),
                title: title.to_owned(),
                listeners: RefCell::default(),
            }),
        };
        let weak = Rc::downgrade(&swatch.inner);
        swatch.inner.area.set_draw_func(glib::clone!(
            #[strong]
            weak,
            move |_, cr, w, h| {
                if let Some(inner) = weak.upgrade() {
                    draw_swatch(cr, f64::from(w), f64::from(h), inner.color.get());
                }
            }
        ));
        swatch.inner.button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                ColorSwatch { inner }.pick();
            }
        });
        swatch.refresh();
        swatch
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.inner.button.upcast_ref()
    }

    pub fn color(&self) -> Color {
        self.inner.color.get()
    }

    /// Sets the color without notifying listeners.
    pub fn set_color(&self, color: Color) {
        if self.inner.color.replace(color) != color {
            self.refresh();
        }
    }

    /// Calls `f` after the user picked a new color.
    pub fn connect_changed(&self, f: impl Fn(Color) + 'static) {
        self.inner.listeners.borrow_mut().push(Rc::new(f));
    }

    fn refresh(&self) {
        let c = self.color();
        self.inner
            .button
            .set_tooltip_text(Some(&format!("{}  {}", self.inner.title, c.to_hex())));
        self.inner.area.queue_draw();
    }

    fn pick(&self) {
        let dialog = gtk::ColorDialog::builder()
            .title(self.inner.title.as_str())
            .with_alpha(true)
            .modal(true)
            .build();
        let parent = self
            .inner
            .button
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok());
        let (r, g, b, a) = self.color().rgba_f64();
        let initial = gdk::RGBA::new(r as f32, g as f32, b as f32, a as f32);
        let weak: Weak<Inner> = Rc::downgrade(&self.inner);
        dialog.choose_rgba(
            parent.as_ref(),
            Some(&initial),
            None::<&gio::Cancellable>,
            move |result| {
                let (Ok(rgba), Some(inner)) = (result, weak.upgrade()) else {
                    return;
                };
                let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                let color = Color {
                    r: ch(rgba.red()),
                    g: ch(rgba.green()),
                    b: ch(rgba.blue()),
                    a: ch(rgba.alpha()),
                };
                let swatch = ColorSwatch { inner };
                if color != swatch.color() {
                    swatch.set_color(color);
                    let listeners = swatch.inner.listeners.borrow().clone();
                    for f in listeners {
                        f(color);
                    }
                }
            },
        );
    }
}

/// The color over a checkerboard (visible when translucent); the right
/// third shows the opaque color for reference.
fn draw_swatch(cr: &cairo::Context, w: f64, h: f64, color: Color) {
    const CHECK: f64 = 5.0;
    if color.a < 255 {
        let mut y = 0.0;
        let mut row = 0;
        while y < h {
            let mut x = 0.0;
            let mut col = row;
            while x < w {
                let v = if col % 2 == 0 { 0.62 } else { 0.38 };
                cr.set_source_rgb(v, v, v);
                cr.rectangle(x, y, CHECK, CHECK);
                let _ = cr.fill();
                x += CHECK;
                col += 1;
            }
            y += CHECK;
            row += 1;
        }
    }
    let (r, g, b, a) = color.rgba_f64();
    cr.set_source_rgba(r, g, b, a);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
    if color.a < 255 {
        cr.set_source_rgb(r, g, b);
        cr.rectangle((w * 2.0 / 3.0).round(), 0.0, w, h);
        let _ = cr.fill();
    }
}
