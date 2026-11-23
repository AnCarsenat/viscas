//! Blender-style property rows: a right-aligned label in the left column,
//! the editing widget in the right column.

use gtk::prelude::*;

fn row_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .homogeneous(true)
        .spacing(8)
        .css_classes(["prop-row"])
        .build()
}

/// A row with `label` on the left and `widget` on the right.
pub fn property_row(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = row_box();
    let text = gtk::Label::builder()
        .label(label)
        .xalign(1.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["prop-label"])
        .build();
    row.append(&text);
    widget.set_hexpand(true);
    widget.set_valign(gtk::Align::Center);
    row.append(widget);
    row
}

/// A checkbox row: empty left column, the checkbox and its label on the
/// right (like Blender's boolean properties). Returns the row and button.
pub fn check_row(label: &str, active: bool) -> (gtk::Box, gtk::CheckButton) {
    let check = gtk::CheckButton::builder()
        .label(label)
        .active(active)
        .build();
    let row = row_box();
    row.append(&gtk::Box::new(gtk::Orientation::Horizontal, 0));
    row.append(&check);
    (row, check)
}
