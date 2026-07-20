//! Renders LSP hover content in the minibuffer for the vim `ShowHover` action
//! (`shift-k`), showing the same type signature and documentation the floating
//! hover popover does, but in the bottom-anchored minibuffer strip instead.

use editor::hover_popover::{hover_markdown_style, open_markdown_url};
use gpui::{
    Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, ScrollHandle, Subscription,
    WeakEntity, Window,
};
use markdown::{
    CodeBlockRenderer, CopyButtonVisibility, Markdown, MarkdownElement, WrapButtonVisibility,
};
use ui::prelude::*;
use workspace::Workspace;

pub struct HoverView {
    focus_handle: FocusHandle,
    markdown: Entity<Markdown>,
    scroll_handle: ScrollHandle,
    workspace: WeakEntity<Workspace>,
    _subscription: Subscription,
}

impl HoverView {
    pub fn new(
        markdown: Option<Entity<Markdown>>,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let markdown = markdown.unwrap_or_else(|| {
            cx.new(|cx| Markdown::new("No hover information".into(), None, None, cx))
        });
        let subscription = cx.observe(&markdown, |_, _, cx| cx.notify());
        Self {
            focus_handle: cx.focus_handle(),
            markdown,
            scroll_handle: ScrollHandle::new(),
            workspace,
            _subscription: subscription,
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl Render for HoverView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self.workspace.clone();
        div()
            .id("hover-view")
            .key_context("HoverView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .p_2()
            .child(
                MarkdownElement::new(self.markdown.clone(), hover_markdown_style(window, cx))
                    .scroll_handle(self.scroll_handle.clone())
                    .code_block_renderer(CodeBlockRenderer::Default {
                        copy_button_visibility: CopyButtonVisibility::Hidden,
                        wrap_button_visibility: WrapButtonVisibility::Hidden,
                        border: false,
                    })
                    .on_url_click(move |link, window, cx| {
                        open_markdown_url(workspace.upgrade(), link, window, cx);
                    }),
            )
    }
}

impl Focusable for HoverView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for HoverView {}
