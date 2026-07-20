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
        /// Resets the index only to a revision, keeping the working tree.
        ResetIndex,
        /// Resets the working tree only to a revision, keeping the index, discarding changes.
        ResetWorktree,
        /// Resets the selected file (index and working tree) to a revision, discarding changes.
        ResetFile,
        /// Picks the reset target from the branch list instead of typing it.
        ResetFromBranch,
    ]
);

/// Which kind of reset the transient will run once a revision is entered.
#[derive(Clone, Copy)]
enum ResetKind {
    /// A whole-tree `git reset` with the given mode.
    Mode(ResetMode),
    /// Restore the index only (`git restore --staged`).
    Index,
    /// Restore the working tree only (`git restore --worktree`).
    Worktree,
    /// Restore the selected file's index and working tree (`git restore --staged --worktree`).
    File,
}

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
    /// `None` while the kind is still being chosen, `Some` once the popup is
    /// waiting for the target revision.
    kind: Option<ResetKind>,
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
            kind: None,
            editor,
            focus_handle: cx.focus_handle(),
        }
    }

    fn choose_kind(&mut self, kind: ResetKind, window: &mut Window, cx: &mut Context<Self>) {
        self.kind = Some(kind);
        window.focus(&self.editor.focus_handle(cx), cx);
        cx.notify();
    }

    fn reset_mixed(&mut self, _: &ResetMixed, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_kind(ResetKind::Mode(ResetMode::Mixed), window, cx);
    }

    fn reset_soft(&mut self, _: &ResetSoft, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_kind(ResetKind::Mode(ResetMode::Soft), window, cx);
    }

    fn reset_hard(&mut self, _: &ResetHard, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_kind(ResetKind::Mode(ResetMode::Hard), window, cx);
    }

    fn reset_index(&mut self, _: &ResetIndex, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_kind(ResetKind::Index, window, cx);
    }

    fn reset_worktree(&mut self, _: &ResetWorktree, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_kind(ResetKind::Worktree, window, cx);
    }

    fn reset_file(&mut self, _: &ResetFile, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_kind(ResetKind::File, window, cx);
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.kind.take() else {
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
            .update(cx, |panel, cx| match kind {
                ResetKind::Mode(mode) => panel.reset_to(revision, mode, window, cx),
                ResetKind::Index => {
                    panel.restore_to(revision, true, false, Vec::new(), window, cx)
                }
                ResetKind::Worktree => {
                    panel.restore_to(revision, false, true, Vec::new(), window, cx)
                }
                ResetKind::File => panel.restore_selected_file_to(revision, window, cx),
            })
            .ok();
        cx.emit(DismissEvent);
    }

    /// Replaces the revision input with the branch list in select mode; the
    /// branch the user picks becomes the reset target, run with the kind already
    /// chosen. The hard-reset confirmation still happens inside `reset_to`.
    fn reset_from_branch(
        &mut self,
        _: &ResetFromBranch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = self.kind.take() else {
            return;
        };
        let panel = self.panel.clone();
        let workspace = self.workspace.clone();
        let on_select: SelectBranchCallback = Arc::new(move |branch, window, cx| {
            let revision = branch.name().to_string();
            panel
                .update(cx, |panel, cx| match kind {
                    ResetKind::Mode(mode) => panel.reset_to(revision, mode, window, cx),
                    ResetKind::Index => {
                        panel.restore_to(revision, true, false, Vec::new(), window, cx)
                    }
                    ResetKind::Worktree => {
                        panel.restore_to(revision, false, true, Vec::new(), window, cx)
                    }
                    ResetKind::File => panel.restore_selected_file_to(revision, window, cx),
                })
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

    fn kind_label(kind: &ResetKind) -> &'static str {
        match kind {
            ResetKind::Mode(ResetMode::Soft) => "soft",
            ResetKind::Mode(ResetMode::Mixed) => "mixed",
            ResetKind::Mode(ResetMode::Hard) => "hard",
            ResetKind::Index => "index",
            ResetKind::Worktree => "worktree",
            ResetKind::File => "file",
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
        let key_context = if self.kind.is_some() {
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
            .on_action(cx.listener(Self::reset_index))
            .on_action(cx.listener(Self::reset_worktree))
            .on_action(cx.listener(Self::reset_file))
            .on_action(cx.listener(Self::reset_from_branch))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .p_3()
            .gap_1()
            .map(|this| match self.kind.as_ref() {
                Some(kind) => this
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Label::new(format!("Reset {} to:", Self::kind_label(kind)))
                                    .color(Color::Muted),
                            )
                            .child(div().flex_1().child(self.editor.clone())),
                    )
                    .child(render_action("cmd-b", "pick from branch list")),
                None => this
                    .child(render_section("Reset"))
                    .child(render_action("m", "mixed     (--mixed)"))
                    .child(render_action("s", "soft      (--soft)"))
                    .child(render_action("h", "hard      (--hard, discards changes)"))
                    .child(render_action("i", "index     (index only)"))
                    .child(render_action("w", "worktree  (worktree only, discards changes)"))
                    .child(render_action("f", "file      (selected file, discards changes)")),
            })
    }
}
