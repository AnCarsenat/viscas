//! One scene tree row: expander, kind icon, name (or inline rename
//! entry), detail, status dot and the visibility / lock toggles.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use viscas_engine::Engine;
use viscas_scene::{KindTag, NodeId, NodeKind};

use super::model::{self, Dot};
use crate::editors::pick_icon;

const EYE_ON: &[&str] = &["view-reveal-symbolic", "eye-open-negative-filled-symbolic"];
const EYE_OFF: &[&str] = &["view-conceal-symbolic", "eye-not-looking-symbolic"];
const LOCK_ON: &[&str] = &["changes-prevent-symbolic", "system-lock-screen-symbolic"];
const LOCK_OFF: &[&str] = &["changes-allow-symbolic", "object-unlocked-symbolic"];
const FOLDER: &[&str] = &["folder-symbolic", "folder"];

/// Widgets of one row; reused by the list view for different nodes.
pub struct Row {
    pub expander: gtk::TreeExpander,
    glyph: gtk::Label,
    folder: gtk::Image,
    pub stack: gtk::Stack,
    pub title: gtk::Label,
    detail: gtk::Label,
    pub entry: gtk::Entry,
    pub error: gtk::Label,
    dot: gtk::Label,
    pub eye: gtk::Button,
    pub lock: gtk::Button,
    pub id: Cell<Option<NodeId>>,
    pub list_row: RefCell<Option<gtk::TreeListRow>>,
    kind_class: RefCell<String>,
}

fn toggle(tip: &str) -> gtk::Button {
    gtk::Button::builder()
        .css_classes(["tree-toggle", "flat"])
        .tooltip_text(tip)
        .focusable(false)
        .valign(gtk::Align::Center)
        .build()
}

impl Row {
    pub fn new() -> Row {
        let glyph = gtk::Label::builder()
            .css_classes(["kind-glyph"])
            .width_chars(2)
            .build();
        let folder = gtk::Image::builder()
            .icon_name(pick_icon(FOLDER))
            .css_classes(["kind-glyph", "kind-folder"])
            .visible(false)
            .build();
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["tree-title"])
            .build();
        let detail = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["tree-detail", "mono"])
            .build();
        let label_box = gtk::Box::builder().spacing(6).hexpand(true).build();
        label_box.append(&title);
        label_box.append(&detail);
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .css_classes(["tree-rename"])
            .build();
        let stack = gtk::Stack::builder()
            .hexpand(true)
            .hhomogeneous(false)
            .build();
        stack.add_named(&label_box, Some("label"));
        stack.add_named(&entry, Some("edit"));
        let error = gtk::Label::builder()
            .css_classes(["rename-error"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(28)
            .visible(false)
            .build();
        let dot = gtk::Label::builder()
            .label("●")
            .css_classes(["status-dot-tree"])
            .build();
        let eye = toggle("Visibility");
        let lock = toggle("Lock");
        let content = gtk::Box::builder()
            .spacing(4)
            .css_classes(["tree-row-content"])
            .build();
        content.append(&glyph);
        content.append(&folder);
        content.append(&stack);
        content.append(&error);
        content.append(&dot);
        content.append(&eye);
        content.append(&lock);
        let expander = gtk::TreeExpander::builder()
            .child(&content)
            .indent_for_icon(true)
            .css_classes(["tree-row"])
            .build();
        Row {
            expander,
            glyph,
            folder,
            stack,
            title,
            detail,
            entry,
            error,
            dot,
            eye,
            lock,
            id: Cell::new(None),
            list_row: RefCell::default(),
            kind_class: RefCell::default(),
        }
    }

    pub fn is_renaming(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some("edit")
    }

    /// Updates every widget from the engine's current state.
    pub fn refresh(&self, engine: &Engine) {
        let Some(id) = self.id.get() else {
            return;
        };
        let Some(node) = engine.scene().get(id) else {
            return;
        };
        let tag = node.kind.tag();
        let is_folder = tag == KindTag::Folder;
        self.glyph.set_visible(!is_folder);
        self.folder.set_visible(is_folder);
        self.glyph.set_label(model::kind_glyph(tag));
        let class = format!("kind-{}", tag.as_str());
        let old = self.kind_class.replace(class.clone());
        if old != class {
            if !old.is_empty() {
                self.glyph.remove_css_class(&old);
            }
            self.glyph.add_css_class(&class);
        }
        self.glyph.set_tooltip_text(Some(model::kind_title(tag)));
        self.folder.set_tooltip_text(Some(model::kind_title(tag)));

        let approx = engine.settings().approx;
        let value = engine.value(id).map(|v| v.display(approx).to_owned());
        let value = value.filter(|_| {
            matches!(
                node.kind,
                NodeKind::Variable { .. } | NodeKind::Slider { .. } | NodeKind::List { .. }
            ) || matches!(node.kind, NodeKind::Point { .. })
        });
        let text = model::row_text(node, value.as_deref());
        self.title.set_label(&text.title);
        if text.title_is_source {
            self.title.add_css_class("mono");
        } else {
            self.title.remove_css_class("mono");
        }
        self.detail.set_label(text.detail.as_deref().unwrap_or(""));
        self.detail.set_visible(text.detail.is_some());
        let tip = match &node.name {
            Some(_) => format!("{} · {}", model::kind_title(tag), model::search_text(node)),
            None => text.title.clone(),
        };
        self.title.set_tooltip_text(Some(&tip));

        let scene = engine.scene();
        for c in ["ok", "error", "pending"] {
            self.dot.remove_css_class(c);
        }
        if model::has_status(tag) {
            self.dot.set_visible(true);
            let (class, tip) = match model::status_dot(scene, id) {
                Dot::Ok => ("ok", "Up to date".to_owned()),
                Dot::Pending => ("pending", "Pending evaluation".to_owned()),
                Dot::Error(msg) => ("error", msg),
            };
            self.dot.add_css_class(class);
            self.dot.set_tooltip_text(Some(&tip));
        } else {
            self.dot.set_visible(false);
        }

        let set_toggle = |b: &gtk::Button,
                          on: bool,
                          icons: (&'static [&'static str], &'static [&'static str]),
                          tip: (&str, &str)| {
            b.set_icon_name(pick_icon(if on { icons.0 } else { icons.1 }));
            b.set_tooltip_text(Some(if on { tip.0 } else { tip.1 }));
            if on {
                b.add_css_class("on");
            } else {
                b.remove_css_class("on");
            }
        };
        set_toggle(
            &self.eye,
            node.visible,
            (EYE_ON, EYE_OFF),
            ("Visible — click to hide", "Hidden — click to show"),
        );
        set_toggle(
            &self.lock,
            node.locked,
            (LOCK_ON, LOCK_OFF),
            ("Locked — click to unlock", "Unlocked — click to lock"),
        );
        if node.visible {
            self.expander.remove_css_class("hidden-node");
        } else {
            self.expander.add_css_class("hidden-node");
        }
    }

    /// Shows the inline rename entry.
    pub fn begin_rename(&self, current: &str) {
        self.entry.set_text(current);
        self.stack.set_visible_child_name("edit");
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    /// Back to the label; clears any inline error.
    pub fn end_rename(&self) {
        self.set_error(None);
        self.stack.set_visible_child_name("label");
    }

    pub fn set_error(&self, message: Option<&str>) {
        match message {
            Some(m) => {
                self.error.set_label(m);
                self.error.set_tooltip_text(Some(m));
                self.error.set_visible(true);
                self.entry.add_css_class("error");
            }
            None => {
                self.error.set_visible(false);
                self.entry.remove_css_class("error");
            }
        }
    }

    /// Sets exactly one of `classes` (or none) on the row.
    pub fn set_class_from(&self, classes: &[&str], on: Option<&str>) {
        for c in classes {
            if Some(*c) != on {
                self.expander.remove_css_class(c);
            }
        }
        if let Some(c) = on {
            self.expander.add_css_class(c);
        }
    }
}
