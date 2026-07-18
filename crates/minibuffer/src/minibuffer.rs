//! A transient panel that opens from the bottom of the workspace.
//!
//! The minibuffer is a bottom-anchored, fixed-height surface meant to host a
//! variety of features later on. It is rendered by the workspace as a full-width
//! strip below the status bar (see `Workspace::set_bottom_panel`), so opening it
//! reflows the status bar and center upward instead of covering them.
//!
//! In this iteration it only opens and closes: `minibuffer::Toggle` opens it (or
//! closes it if already open), and Escape dismisses it.

use gpui::{
    App, Context, FocusHandle, Focusable, Pixels, Render, WeakEntity, Window, actions, px,
};
use ui::prelude::*;
use workspace::Workspace;

/// Fixed height of the minibuffer panel. Provisional until features that live in
/// the minibuffer need to size it themselves.
const MINIBUFFER_HEIGHT: Pixels = px(400.);

actions!(
    minibuffer,
    [
        /// Toggles the minibuffer panel open or closed.
        Toggle
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(toggle);
    })
    .detach();
}

fn toggle(workspace: &mut Workspace, _: &Toggle, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(minibuffer) = workspace
        .bottom_panel()
        .and_then(|panel| panel.clone().downcast::<MiniBuffer>().ok())
    {
        let previous_focus = minibuffer.read(cx).previous_focus.clone();
        workspace.clear_bottom_panel(cx);
        if let Some(previous_focus) = previous_focus {
            previous_focus.focus(window, cx);
        }
        return;
    }

    let workspace_handle = cx.weak_entity();
    let minibuffer = cx.new(|cx| MiniBuffer::new(workspace_handle, window, cx));
    let focus_handle = minibuffer.read(cx).focus_handle.clone();
    workspace.set_bottom_panel(minibuffer, cx);
    window.focus(&focus_handle, cx);
}

pub struct MiniBuffer {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    /// Focus to restore when the minibuffer closes, captured at open time.
    previous_focus: Option<FocusHandle>,
}

impl MiniBuffer {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            previous_focus: window.focused(cx),
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        let previous_focus = self.previous_focus.take();
        self.workspace
            .update(cx, |workspace, cx| workspace.clear_bottom_panel(cx))
            .ok();
        if let Some(previous_focus) = previous_focus {
            previous_focus.focus(window, cx);
        }
    }
}

impl Render for MiniBuffer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("MiniBuffer")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::cancel))
            .w_full()
            .h(MINIBUFFER_HEIGHT)
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().elevated_surface_background)
    }
}

impl Focusable for MiniBuffer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
