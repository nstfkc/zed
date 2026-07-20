//! A magit-style transient popup for `git rebase`, rendered in the workspace's
//! minibuffer. It can rebase onto the current branch's upstream or onto a branch
//! picked from a list, and drives an in-progress rebase (continue/skip/abort).

use std::sync::Arc;

use git::repository::{RebaseAction, RebaseArgs};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, WeakEntity, Window,
    actions,
};
use ui::prelude::*;
use util::ResultExt as _;
use workspace::Workspace;

use crate::branch_picker::{self, SelectBranchCallback};
use crate::git_panel::GitPanel;
use crate::transient::{render_action, render_argument, render_section};

actions!(
    git_rebase_popup,
    [
        /// Toggles the `--autostash` rebase argument.
        ToggleAutostash,
        /// Toggles the `--autosquash` rebase argument.
        ToggleAutosquash,
        /// Rebases the current branch onto its upstream.
        RebaseOntoUpstream,
        /// Rebases the current branch onto a branch picked from a list.
        RebaseOntoElsewhere,
        /// Continues an in-progress rebase.
        RebaseContinue,
        /// Skips the current commit of an in-progress rebase.
        RebaseSkip,
        /// Aborts an in-progress rebase.
        RebaseAbort,
    ]
);

/// Opens the rebase transient in the minibuffer for the given Git panel.
pub fn open(
    workspace: &mut Workspace,
    panel: Entity<GitPanel>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let repository = workspace.project().read(cx).active_repository(cx);
    let branch = repository.and_then(|repo| repo.read(cx).branch.clone());
    let onto_branch: SharedString = branch
        .as_ref()
        .map(|branch| branch.name().to_string().into())
        .unwrap_or_else(|| "HEAD".into());
    let upstream: Option<SharedString> = branch
        .as_ref()
        .and_then(|branch| branch.upstream.as_ref())
        .map(|upstream| {
            upstream
                .stripped_ref_name()
                .unwrap_or(upstream.ref_name.as_ref())
                .to_string()
                .into()
        });

    let workspace_handle = workspace.weak_handle();
    let popup = cx.new(|cx| {
        RebasePopup::new(
            panel.downgrade(),
            workspace_handle,
            onto_branch,
            upstream,
            cx,
        )
    });
    minibuffer::show(workspace, popup, window, cx);
}

pub struct RebasePopup {
    panel: WeakEntity<GitPanel>,
    workspace: WeakEntity<Workspace>,
    args: RebaseArgs,
    branch: SharedString,
    upstream: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl RebasePopup {
    fn new(
        panel: WeakEntity<GitPanel>,
        workspace: WeakEntity<Workspace>,
        branch: SharedString,
        upstream: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            panel,
            workspace,
            args: RebaseArgs::default(),
            branch,
            upstream,
            focus_handle: cx.focus_handle(),
        }
    }

    fn toggle_autostash(&mut self, _: &ToggleAutostash, _: &mut Window, cx: &mut Context<Self>) {
        self.args.autostash = !self.args.autostash;
        cx.notify();
    }

    fn toggle_autosquash(&mut self, _: &ToggleAutosquash, _: &mut Window, cx: &mut Context<Self>) {
        self.args.autosquash = !self.args.autosquash;
        cx.notify();
    }

    fn run(&mut self, action: RebaseAction, window: &mut Window, cx: &mut Context<Self>) {
        let args = self.args;
        self.panel
            .update(cx, |panel, cx| panel.rebase(action, args, window, cx))
            .ok();
        cx.emit(DismissEvent);
    }

    fn onto_upstream(
        &mut self,
        _: &RebaseOntoUpstream,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(upstream) = self.upstream.clone() else {
            return;
        };
        self.run(RebaseAction::Onto(upstream.to_string()), window, cx);
    }

    fn onto_elsewhere(
        &mut self,
        _: &RebaseOntoElsewhere,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.panel.clone();
        let workspace = self.workspace.clone();
        let args = self.args;
        let on_select: SelectBranchCallback = Arc::new(move |branch, window, cx| {
            let onto = branch.name().to_string();
            panel
                .update(cx, |panel, cx| {
                    panel.rebase(RebaseAction::Onto(onto), args, window, cx)
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

    fn continue_rebase(&mut self, _: &RebaseContinue, window: &mut Window, cx: &mut Context<Self>) {
        self.run(RebaseAction::Continue, window, cx);
    }

    fn skip(&mut self, _: &RebaseSkip, window: &mut Window, cx: &mut Context<Self>) {
        self.run(RebaseAction::Skip, window, cx);
    }

    fn abort(&mut self, _: &RebaseAbort, window: &mut Window, cx: &mut Context<Self>) {
        self.run(RebaseAction::Abort, window, cx);
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl Focusable for RebasePopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for RebasePopup {}

impl Render for RebasePopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let upstream = self
            .upstream
            .clone()
            .unwrap_or_else(|| "no upstream configured".into());

        v_flex()
            .key_context("GitRebasePopup")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_autostash))
            .on_action(cx.listener(Self::toggle_autosquash))
            .on_action(cx.listener(Self::onto_upstream))
            .on_action(cx.listener(Self::onto_elsewhere))
            .on_action(cx.listener(Self::continue_rebase))
            .on_action(cx.listener(Self::skip))
            .on_action(cx.listener(Self::abort))
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .p_3()
            .gap_1()
            .child(render_section("Arguments"))
            .child(render_argument(
                "-a",
                "Autostash",
                "(--autostash)",
                self.args.autostash,
            ))
            .child(render_argument(
                "-s",
                "Autosquash",
                "(--autosquash)",
                self.args.autosquash,
            ))
            .child(div().h_2())
            .child(
                h_flex()
                    .gap_1()
                    .child(Label::new("Rebase").color(Color::Muted))
                    .child(Label::new(self.branch.clone()).color(Color::Info))
                    .child(Label::new("onto").color(Color::Muted)),
            )
            .child(render_action("u", upstream))
            .child(render_action("e", "elsewhere"))
            .child(div().h_2())
            .child(render_section("In progress"))
            .child(render_action("r", "Continue"))
            .child(render_action("s", "Skip"))
            .child(render_action("a", "Abort"))
    }
}
