use command_palette_hooks::GlobalCommandPaletteInterceptor;
use editor::{Editor, EditorEvent};
use gpui::{
    AbsoluteLength, App, Context, Entity, FocusHandle, Focusable, Render, SharedString,
    Subscription, Task, TextStyleRefinement, WeakEntity, Window, actions, div, px, rems,
};
use project::Hover;
use ui::prelude::*;
use workspace::{HideStatusItem, StatusItemView, Workspace, item::ItemHandle};

actions!(
    minibuffer,
    [
        /// Starts an Ex-style command line in the minibuffer (the `:` prompt).
        StartCommand,
        /// Shows LSP type/hover information for the symbol under the cursor in the minibuffer.
        ShowTypeInfo,
    ]
);

/// Registers the minibuffer actions on every workspace. The `MiniBuffer`
/// status-bar item itself is added to the status bar by the `zed` crate.
pub fn init(cx: &mut App) {
    cx.observe_new(MiniBuffer::register).detach();
}

#[derive(PartialEq, Eq)]
enum Mode {
    Idle,
    Command,
    TypeInfo,
}

/// A neovim/doom-emacs style minibuffer that lives at the bottom of the window
/// as a status-bar item. It hosts the Ex command line and displays type info.
pub struct MiniBuffer {
    input: Entity<Editor>,
    mode: Mode,
    message: Option<SharedString>,
    workspace: WeakEntity<Workspace>,
    previous_focus: Option<FocusHandle>,
    pending_command: Task<()>,
    pending_type_info: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl MiniBuffer {
    /// Constructs the minibuffer status-bar item for the given workspace.
    pub fn new(workspace: &Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_read_only(false);
            // Use a compact font so the command line doesn't make the status
            // bar taller than the rest of its items.
            editor.set_text_style_refinement(TextStyleRefinement {
                font_size: Some(AbsoluteLength::Pixels(px(12.))),
                ..Default::default()
            });
            editor
        });

        let input_subscription = cx.subscribe(&input, Self::on_input_event);

        Self {
            input,
            mode: Mode::Idle,
            message: None,
            workspace: workspace.weak_handle(),
            previous_focus: None,
            pending_command: Task::ready(()),
            pending_type_info: Task::ready(()),
            _subscriptions: vec![input_subscription],
        }
    }

    fn register(
        workspace: &mut Workspace,
        _window: Option<&mut Window>,
        _cx: &mut Context<Workspace>,
    ) {
        workspace.register_action(|workspace, _: &StartCommand, window, cx| {
            let Some(minibuffer) = workspace.status_bar().read(cx).item_of_type::<MiniBuffer>()
            else {
                return;
            };
            minibuffer.update(cx, |minibuffer, cx| {
                minibuffer.start_command(window, cx);
            });
        });

        workspace.register_action(|workspace, _: &ShowTypeInfo, window, cx| {
            let Some(minibuffer) = workspace.status_bar().read(cx).item_of_type::<MiniBuffer>()
            else {
                return;
            };
            minibuffer.update(cx, |minibuffer, cx| {
                minibuffer.show_type_info(window, cx);
            });
        });
    }

    fn start_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Command;
        self.message = None;
        self.previous_focus = window.focused(cx);
        self.input.update(cx, |editor, cx| {
            editor.set_read_only(false);
            editor.set_text("", window, cx);
        });
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn exit(&mut self, restore_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Idle;
        self.input.update(cx, |editor, cx| {
            editor.set_read_only(false);
            editor.set_text("", window, cx);
        });
        if restore_focus && let Some(previous) = self.previous_focus.take() {
            window.focus(&previous, cx);
        } else {
            self.previous_focus = None;
        }
        cx.notify();
    }

    fn on_input_event(
        &mut self,
        _: Entity<Editor>,
        event: &EditorEvent,
        cx: &mut Context<Self>,
    ) {
        // When the input loses focus while a prompt is active, quietly dismiss
        // it. Confirm/cancel switch to `Idle` before moving focus, so those
        // paths are not affected by this handler.
        if let EditorEvent::Blurred = event
            && self.mode != Mode::Idle
        {
            self.mode = Mode::Idle;
            self.previous_focus = None;
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Command {
            return;
        }

        let query = self.input.read(cx).text(cx);
        let query = query.trim().to_string();
        if query.is_empty() {
            self.exit(true, window, cx);
            return;
        }

        let target = self.previous_focus.clone();
        let intercept = GlobalCommandPaletteInterceptor::intercept(&query, self.workspace.clone(), cx);

        self.mode = Mode::Idle;
        self.input.update(cx, |editor, cx| editor.set_text("", window, cx));
        if let Some(target) = &target {
            window.focus(target, cx);
        }
        cx.notify();

        self.pending_command = cx.spawn_in(window, async move |this, cx| {
            let result = match intercept {
                Some(task) => task.await,
                None => Default::default(),
            };
            this.update_in(cx, |this, window, cx| {
                if let Some(item) = result.results.into_iter().next() {
                    this.message = None;
                    if let Some(target) = &target {
                        target.dispatch_action(item.action.as_ref(), window, cx);
                    } else {
                        window.dispatch_action(item.action, cx);
                    }
                } else {
                    this.message = Some(format!("Not an editor command: {query}").into());
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Idle {
            return;
        }
        self.message = None;
        self.exit(true, window, cx);
    }

    fn show_type_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self
            .workspace
            .upgrade()
            .and_then(|workspace| workspace.read(cx).active_item(cx))
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            self.message = Some("No active editor".into());
            cx.notify();
            return;
        };

        let hover_request = editor.update(cx, |editor, cx| {
            let project = editor.project()?.clone();
            let multibuffer = editor.buffer().read(cx);
            let snapshot = multibuffer.snapshot(cx);
            let head = editor.selections.newest_anchor().head();
            let (buffer_position, _) = snapshot.anchor_to_buffer_anchor(head)?;
            let buffer = multibuffer.buffer(buffer_position.buffer_id)?;
            Some(project.update(cx, |project, cx| project.hover(&buffer, buffer_position, cx)))
        });

        let Some(hover_request) = hover_request else {
            self.message = Some("No type info available".into());
            cx.notify();
            return;
        };

        self.previous_focus = window.focused(cx);
        self.pending_type_info = cx.spawn_in(window, async move |this, cx| {
            let hovers = hover_request.await.unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                let text = Self::hover_text(&hovers);
                match text {
                    Some(text) => {
                        this.mode = Mode::TypeInfo;
                        this.message = None;
                        this.input.update(cx, |editor, cx| {
                            editor.set_read_only(false);
                            editor.set_text(text, window, cx);
                            editor.set_read_only(true);
                        });
                        this.input.focus_handle(cx).focus(window, cx);
                    }
                    None => {
                        this.mode = Mode::Idle;
                        this.message = Some("No type info available".into());
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn hover_text(hovers: &[Hover]) -> Option<SharedString> {
        let text = hovers
            .iter()
            .flat_map(|hover| hover.contents.iter())
            .map(|block| block.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            None
        } else {
            Some(text.into())
        }
    }
}

impl Focusable for MiniBuffer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for MiniBuffer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut container = h_flex()
            .key_context("MiniBuffer")
            .pl_2()
            .gap_1()
            .min_w_0()
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel));

        match self.mode {
            Mode::Command => {
                container = container
                    .child(Label::new(":"))
                    .child(div().flex_1().min_w(rems(16.)).child(self.input.clone()));
            }
            Mode::TypeInfo => {
                container = container
                    .child(div().flex_1().min_w(rems(16.)).child(self.input.clone()));
            }
            Mode::Idle => {
                if let Some(message) = self.message.clone() {
                    container = container.child(Label::new(message).color(Color::Muted));
                }
            }
        }

        container
    }
}

impl StatusItemView for MiniBuffer {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _cx: &App) -> Option<HideStatusItem> {
        None
    }
}
