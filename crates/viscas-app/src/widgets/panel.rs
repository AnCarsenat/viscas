//! Collapsible panel with a disclosure triangle, like Blender's panels.

use gtk::glib;
use gtk::prelude::*;

/// A titled panel whose body folds away. Cloning shares the widgets.
#[derive(Clone)]
pub struct Panel {
    root: gtk::Box,
    body: gtk::Box,
}

impl Panel {
    pub fn new(title: &str, expanded: bool) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["panel"])
            .build();
        let arrow = gtk::Image::from_icon_name(arrow_icon(expanded));
        arrow.add_css_class("panel-arrow");
        let header_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        header_content.append(&arrow);
        header_content.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["panel-title"])
                .build(),
        );
        let header = gtk::Button::builder()
            .child(&header_content)
            .css_classes(["panel-header"])
            .build();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .css_classes(["panel-body"])
            .build();
        let revealer = gtk::Revealer::builder()
            .child(&body)
            .reveal_child(expanded)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(120)
            .build();
        root.append(&header);
        root.append(&revealer);
        if !expanded {
            root.add_css_class("collapsed");
        }
        header.connect_clicked(glib::clone!(
            #[weak]
            root,
            #[weak]
            arrow,
            #[weak]
            revealer,
            move |_| {
                let open = !revealer.reveals_child();
                revealer.set_reveal_child(open);
                arrow.set_icon_name(Some(arrow_icon(open)));
                if open {
                    root.remove_css_class("collapsed");
                } else {
                    root.add_css_class("collapsed");
                }
            }
        ));
        Self { root, body }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    pub fn append(&self, child: &impl IsA<gtk::Widget>) {
        self.body.append(child);
    }
}

fn arrow_icon(expanded: bool) -> &'static str {
    if expanded {
        "pan-down-symbolic"
    } else {
        "pan-end-symbolic"
    }
}
