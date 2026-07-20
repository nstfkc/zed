//! Shared rendering helpers for the magit-style transient popups that the
//! full-screen Git panel shows in the minibuffer.

use ui::prelude::*;

/// A section heading such as "Arguments" or "Actions".
pub(crate) fn render_section(title: impl Into<SharedString>) -> impl IntoElement {
    Label::new(title.into()).color(Color::Accent)
}

/// A toggleable argument row, e.g. `-f Fast-forward only (--ff-only)`. Inactive
/// arguments are dimmed so the enabled set is readable at a glance.
pub(crate) fn render_argument(
    key: impl Into<SharedString>,
    description: impl Into<SharedString>,
    flag: impl Into<SharedString>,
    active: bool,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(
            Label::new(key.into())
                .color(Color::Created)
                .size(LabelSize::Small),
        )
        .child(Label::new(description.into()).color(if active {
            Color::Default
        } else {
            Color::Muted
        }))
        .child(Label::new(flag.into()).color(if active {
            Color::Accent
        } else {
            Color::Disabled
        }))
}

/// An action row: the key that runs it and what it will act on.
pub(crate) fn render_action(
    key: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(
            Label::new(key.into())
                .color(Color::Error)
                .size(LabelSize::Small),
        )
        .child(Label::new(description.into()).color(Color::Info))
}
