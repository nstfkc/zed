use command_palette_hooks::GlobalCommandPaletteInterceptor;
use editor::{Editor, EditorEvent};
use gpui::{
    Action, App, Context, Entity, FocusHandle, Focusable, FontWeight, Render, Subscription, Task,
    TextStyleRefinement, WeakEntity, Window, actions,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;
use workspace::{HideStatusItem, StatusItemView, Workspace, item::ItemHandle};

actions!(
    vim,
    [
        /// Opens the vim command line in the status bar.
        OpenCommandLine
    ]
);

/// A vim-style command line hosted in the status bar. Pressing `:` opens it in
/// place (instead of the command palette); as the user types, the matched
/// command is previewed inline, `enter` runs it, and `escape` cancels.
///
/// Command parsing and execution reuse the command palette's interceptor, so
/// every `:` command the palette understands works here too.
pub struct VimCommandLine {
    workspace: WeakEntity<Workspace>,
    input: Option<Entity<Editor>>,
    preview: SharedString,
    action: Option<Box<dyn Action>>,
    previous_focus: Option<FocusHandle>,
    input_subscription: Option<Subscription>,
    intercept_task: Task<()>,
}

impl VimCommandLine {
    pub fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self {
            workspace,
            input: None,
            preview: SharedString::default(),
            action: None,
            previous_focus: None,
            input_subscription: None,
            intercept_task: Task::ready(()),
        }
    }

    /// Opens the command line if closed, or closes it if already open.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.is_some() {
            self.close(window, cx);
        } else {
            self.open(window, cx);
        }
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.previous_focus = window.focused(cx);
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("command", window, cx);
            // Match the small UI font so the command line doesn't grow the
            // status bar taller than its other items.
            let font_size = TextSize::Small
                .rems(cx)
                .to_pixels(ThemeSettings::get_global(cx).ui_font_size(cx));
            editor.set_text_style_refinement(TextStyleRefinement {
                font_size: Some(font_size.into()),
                ..Default::default()
            });
            editor
        });
        self.input_subscription = Some(cx.subscribe_in(
            &editor,
            window,
            |this, _editor, event, window, cx| {
                if matches!(event, EditorEvent::BufferEdited) {
                    this.update_preview(window, cx);
                }
            },
        ));
        window.focus(&editor.focus_handle(cx), cx);
        self.input = Some(editor);
        self.preview = SharedString::default();
        self.action = None;
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.is_none() {
            return;
        }
        self.input = None;
        self.input_subscription = None;
        self.preview = SharedString::default();
        self.action = None;
        self.intercept_task = Task::ready(());
        if let Some(previous_focus) = self.previous_focus.take() {
            window.focus(&previous_focus, cx);
        }
        cx.notify();
    }

    fn update_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.input.as_ref() else {
            return;
        };
        let query = input.read(cx).text(cx);
        if query.is_empty() {
            self.preview = SharedString::default();
            self.action = None;
            cx.notify();
            return;
        }
        let Some(task) =
            GlobalCommandPaletteInterceptor::intercept(&query, self.workspace.clone(), cx)
        else {
            return;
        };
        self.intercept_task = cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Some(item) = result.results.into_iter().next() {
                    this.preview = item.string.into();
                    this.action = Some(item.action);
                } else {
                    this.preview = SharedString::default();
                    this.action = None;
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let action = self.action.take();
        // Restore focus to the editor before dispatching so the command runs in
        // its context (vim commands act on the active editor).
        self.close(window, cx);
        if let Some(action) = action {
            window.dispatch_action(action, cx);
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
    }
}

impl Render for VimCommandLine {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(input) = self.input.clone() else {
            return div().into_any_element();
        };
        h_flex()
            .key_context("VimCommandLine")
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .gap_1()
            .child(
                Label::new(":")
                    .size(LabelSize::Small)
                    .weight(FontWeight::MEDIUM),
            )
            .child(div().min_w(rems(12.)).child(input))
            .when(!self.preview.is_empty(), |el| {
                el.child(
                    Label::new(self.preview.clone())
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
            .into_any_element()
    }
}

impl StatusItemView for VimCommandLine {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}
