//! A magit-style transient popup for `git reset`, rendered in the workspace's
//! minibuffer. Choosing a mode swaps the popup's body for a single-line input
//! where the target revision is typed, so the whole flow stays in one view. The
//! target can also be picked from the branch list instead of being typed.

use std::sync::Arc;

use editor::Editor;
use git::repository::ResetMode;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, WeakEntity, Window,
    actions,
};
use ui::prelude::*;
use util::ResultExt as _;
use workspace::Workspace;

use crate::branch_picker::{self, SelectBranchCallback};
use crate::git_panel::GitPanel;
use crate::transient::{render_action, render_section};

actions!(
    git_reset_popup,
    [
        /// Resets the branch pointer and index, keeping the working tree.
        ResetMixed,
        /// Resets the branch pointer only, keeping the index and working tree.
        ResetSoft,
        /// Resets the branch pointer, index and working tree, discarding changes.
        ResetHard,
        /// Picks the reset target from the branch list instead of typing it.
        ResetFromBranch,
    ]
);

/// Opens the reset transient in the minibuffer for the given Git panel.
pub fn open(
    workspace: &mut Workspace,
    panel: Entity<GitPanel>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let workspace_handle = workspace.weak_handle();
    let popup = cx.new(|cx| ResetPopup::new(panel.downgrade(), workspace_handle, window, cx));
    minibuffer::show(workspace, popup, window, cx);
}

pub struct ResetPopup {
    panel: WeakEntity<GitPanel>,
    workspace: WeakEntity<Workspace>,
    /// `None` while the mode is still being chosen, `Some` once the popup is
    /// waiting for the target revision.
    mode: Option<ResetMode>,
    editor: Entity<Editor>,
    focus_handle: FocusHandle,
}

impl ResetPopup {
    fn new(
        panel: WeakEntity<GitPanel>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Revision (default: HEAD)", window, cx);
            editor
        });
        Self {
            panel,
            workspace,
            mode: None,
            editor,
            focus_handle: cx.focus_handle(),
        }
    }

    fn choose_mode(&mut self, mode: ResetMode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Some(mode);
        window.focus(&self.editor.focus_handle(cx), cx);
        cx.notify();
    }

    fn reset_mixed(&mut self, _: &ResetMixed, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_mode(ResetMode::Mixed, window, cx);
    }

    fn reset_soft(&mut self, _: &ResetSoft, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_mode(ResetMode::Soft, window, cx);
    }

    fn reset_hard(&mut self, _: &ResetHard, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_mode(ResetMode::Hard, window, cx);
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = self.mode.take() else {
            return;
        };
        let revision = self.editor.read(cx).text(cx);
        let revision = revision.trim();
        let revision = if revision.is_empty() {
            "HEAD".to_string()
        } else {
            revision.to_string()
        };
        self.panel
            .update(cx, |panel, cx| panel.reset_to(revision, mode, window, cx))
            .ok();
        cx.emit(DismissEvent);
    }

    /// Replaces the revision input with the branch list in select mode; the
    /// branch the user picks becomes the reset target, run with the mode already
    /// chosen. The hard-reset confirmation still happens inside `reset_to`.
    fn reset_from_branch(
        &mut self,
        _: &ResetFromBranch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mode) = self.mode.take() else {
            return;
        };
        let panel = self.panel.clone();
        let workspace = self.workspace.clone();
        let on_select: SelectBranchCallback = Arc::new(move |branch, window, cx| {
            let revision = branch.name().to_string();
            panel
                .update(cx, |panel, cx| panel.reset_to(revision, mode, window, cx))
                .ok();
        });

        cx.emit(DismissEvent);
        // Deferred so this popup has left the minibuffer before the branch list
        // replaces it.
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    branch_picker::select_in_minibuffer(workspace, on_select, window, cx);
                })
                .log_err();
        });
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn mode_label(mode: &ResetMode) -> &'static str {
        match mode {
            ResetMode::Soft => "soft",
            ResetMode::Mixed => "mixed",
            ResetMode::Hard => "hard",
        }
    }
}

impl Focusable for ResetPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for ResetPopup {}

impl Render for ResetPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let key_context = if self.mode.is_some() {
            "GitResetPopupInput"
        } else {
            "GitResetPopup"
        };

        v_flex()
            .key_context(key_context)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::reset_mixed))
            .on_action(cx.listener(Self::reset_soft))
            .on_action(cx.listener(Self::reset_hard))
            .on_action(cx.listener(Self::reset_from_branch))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .p_3()
            .gap_1()
            .map(|this| match self.mode.as_ref() {
                Some(mode) => this
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Label::new(format!("Reset {} to:", Self::mode_label(mode)))
                                    .color(Color::Muted),
                            )
                            .child(div().flex_1().child(self.editor.clone())),
                    )
                    .child(render_action("cmd-b", "pick from branch list")),
                None => this
                    .child(render_section("Reset"))
                    .child(render_action("m", "mixed  (--mixed)"))
                    .child(render_action("s", "soft   (--soft)"))
                    .child(render_action("h", "hard   (--hard, discards changes)")),
            })
    }
}
