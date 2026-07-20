//! A transient panel that opens from the bottom of the workspace.
//!
//! The minibuffer is a bottom-anchored, fixed-height surface meant to host a
//! variety of features later on. It is rendered by the workspace as a full-width
//! strip below the status bar (see `Workspace::set_bottom_panel`), so opening it
//! reflows the status bar and center upward instead of covering them.
//!
//! In this iteration it only opens and closes: `minibuffer::Toggle` opens it (or
//! closes it if already open), and Escape dismisses it.
//!
//! Content can be shown in the default fixed-height panel or, by opting into
//! single-line mode, in a strip that is exactly one line of text tall.

use gpui::{
    AnyView, App, Context, DismissEvent, Entity, FocusHandle, Focusable, ManagedView, Pixels,
    Render, Subscription, WeakEntity, Window, actions, px,
};
use ui::prelude::*;
use workspace::Workspace;

/// Fixed height of the minibuffer panel. Provisional until features that live in
/// the minibuffer need to size it themselves.
const MINIBUFFER_HEIGHT: Pixels = px(400.);

/// Height of the minibuffer chrome: one line of text in single-line mode,
/// otherwise the fixed panel height.
fn minibuffer_height(single_line: bool, window: &Window) -> Pixels {
    if single_line {
        window.line_height()
    } else {
        MINIBUFFER_HEIGHT
    }
}

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

/// Shows a dismissable view (typically a picker wrapper) inside the minibuffer,
/// replacing any existing bottom-panel content. The view is focused, and when it
/// emits [`DismissEvent`] the minibuffer closes; focus is restored to whatever
/// was focused beforehand, but only if the view still holds focus (so confirming
/// a picker — which moves focus to what it opened — doesn't steal it back).
pub fn show<V: ManagedView>(
    workspace: &mut Workspace,
    view: Entity<V>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    show_with_options(workspace, view, false, window, cx);
}

/// Same as [`show`], but lets the caller opt into single-line mode, where the
/// minibuffer is only one line tall instead of the full fixed-height panel.
pub fn show_with_options<V: ManagedView>(
    workspace: &mut Workspace,
    view: Entity<V>,
    single_line: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let previous_focus = window.focused(cx);
    let focus_handle = view.focus_handle(cx);
    // The view's focus handle is read again on dismiss rather than captured,
    // because content that changes shape while it's open (see
    // [`set_single_line`]) can move focus to a different handle of its own.
    let subscription = cx.subscribe_in(&view, window, {
        move |workspace, view, _: &DismissEvent, window, cx| {
            let restore_focus = view.focus_handle(cx).contains_focused(window, cx);
            workspace.clear_bottom_panel(cx);
            if restore_focus && let Some(previous_focus) = &previous_focus {
                previous_focus.focus(window, cx);
            }
        }
    });
    let host = cx.new(|_| MinibufferHost {
        content: view.into(),
        single_line,
        _subscription: subscription,
    });
    workspace.set_bottom_panel(host, cx);
    window.focus(&focus_handle, cx);
}

/// Switches the currently shown minibuffer content between the single-line strip
/// and the fixed-height panel, for content that changes shape while it is open
/// (e.g. a command prompt that grows into an output view). Does nothing when the
/// bottom panel holds something other than minibuffer content.
pub fn set_single_line(workspace: &Workspace, single_line: bool, cx: &mut App) {
    let Some(host) = workspace
        .bottom_panel()
        .and_then(|panel| panel.clone().downcast::<MinibufferHost>().ok())
    else {
        return;
    };
    host.update(cx, |host, cx| {
        host.single_line = single_line;
        cx.notify();
    });
}

/// Wraps minibuffer content in the fixed-height bottom-panel chrome. Holds the
/// dismiss subscription so it lives exactly as long as the panel is shown.
struct MinibufferHost {
    content: AnyView,
    single_line: bool,
    _subscription: Subscription,
}

impl Render for MinibufferHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .h(minibuffer_height(self.single_line, window))
            .flex_shrink_0()
            .overflow_hidden()
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().elevated_surface_background)
            .child(self.content.clone())
    }
}

pub struct MiniBuffer {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    /// Focus to restore when the minibuffer closes, captured at open time.
    previous_focus: Option<FocusHandle>,
    single_line: bool,
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
            single_line: false,
        }
    }

    pub fn set_single_line(&mut self, single_line: bool, cx: &mut Context<Self>) {
        self.single_line = single_line;
        cx.notify();
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("MiniBuffer")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::cancel))
            .w_full()
            .h(minibuffer_height(self.single_line, window))
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
