//! Fixed sidebar panels (not scene nodes, see REQUIREMENTS §2).
//!
//! Each panel lives in its own module exposing `build() -> gtk::Widget`.
//! To implement a panel, replace the body of its `build` function; the
//! sidebar picks it up through [`Panel::build`].

mod console;
mod expressions;
mod scene;
mod scripts;
mod sheets;

use adw::prelude::*;

/// The sidebar panels, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Expressions,
    Console,
    Sheets,
    Scripts,
    Scene,
}

impl Panel {
    pub const ALL: [Panel; 5] = [
        Panel::Expressions,
        Panel::Console,
        Panel::Sheets,
        Panel::Scripts,
        Panel::Scene,
    ];

    /// Stable identifier, used as the view stack page name and action target.
    pub fn id(self) -> &'static str {
        match self {
            Panel::Expressions => "expressions",
            Panel::Console => "console",
            Panel::Sheets => "sheets",
            Panel::Scripts => "scripts",
            Panel::Scene => "scene",
        }
    }

    /// Human readable title shown in the view switcher.
    pub fn title(self) -> &'static str {
        match self {
            Panel::Expressions => "Expressions",
            Panel::Console => "Console",
            Panel::Sheets => "Sheets",
            Panel::Scripts => "Scripts",
            Panel::Scene => "Scene",
        }
    }

    /// Candidate symbolic icons, most preferred first. Icon themes differ
    /// (Adwaita vs Breeze, ...), so several names are tried.
    fn icon_candidates(self) -> &'static [&'static str] {
        match self {
            Panel::Expressions => &[
                "view-list-ordered-symbolic",
                "format-list-ordered-symbolic",
                "view-list-symbolic",
            ],
            Panel::Console => &["utilities-terminal-symbolic"],
            Panel::Sheets => &["x-office-spreadsheet-symbolic"],
            Panel::Scripts => &["document-edit-symbolic"],
            Panel::Scene => &[
                "view-list-bullet-symbolic",
                "view-list-tree-symbolic",
                "view-list-details-symbolic",
            ],
        }
    }

    /// Symbolic icon for the panel, picked from the current icon theme.
    pub fn icon_name(self) -> &'static str {
        let candidates = self.icon_candidates();
        let theme = gtk::gdk::Display::default().map(|d| gtk::IconTheme::for_display(&d));
        candidates
            .iter()
            .find(|name| theme.as_ref().is_some_and(|t| t.has_icon(name)))
            .unwrap_or(&candidates[0])
    }

    /// Builds the panel's widget.
    pub fn build(self) -> gtk::Widget {
        match self {
            Panel::Expressions => expressions::build(),
            Panel::Console => console::build(),
            Panel::Sheets => sheets::build(),
            Panel::Scripts => scripts::build(),
            Panel::Scene => scene::build(),
        }
    }
}

/// Builds a view stack holding every panel, one page each.
pub fn build_stack() -> adw::ViewStack {
    let stack = adw::ViewStack::new();
    for panel in Panel::ALL {
        stack.add_titled_with_icon(
            &panel.build(),
            Some(panel.id()),
            panel.title(),
            panel.icon_name(),
        );
    }
    stack
}

/// "Coming soon" page used by panels that are not implemented yet.
fn placeholder(panel: Panel, description: &str) -> gtk::Widget {
    adw::StatusPage::builder()
        .icon_name(panel.icon_name())
        .title("Coming soon")
        .description(description)
        .css_classes(["compact"])
        .vexpand(true)
        .build()
        .upcast()
}

#[cfg(test)]
mod tests {
    use super::Panel;

    #[test]
    fn panel_ids_are_unique() {
        let mut ids: Vec<_> = Panel::ALL.iter().map(|p| p.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Panel::ALL.len());
    }
}
