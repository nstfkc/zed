//! A magit-style transient popup for `git pull`, rendered in the workspace's
//! minibuffer. It lists the pull arguments as toggleable checkboxes (each bound
//! to its own key), shows the branch being pulled into and its upstream, and
//! runs the pull with the selected arguments.

use git::repository::PullArgs;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, WeakEntity, Window,
    actions,
};
use ui::prelude::*;
use workspace::Workspace;

use crate::git_panel::GitPanel;
use crate::transient::{render_action, render_argument, render_section};

actions!(
    git_pull_popup,
    [
        /// Toggles the `--ff-only` pull argument.
        ToggleFfOnly,
        /// Toggles the `--rebase` pull argument.
        ToggleRebase,
        /// Toggles the `--autostash` pull argument.
        ToggleAutostash,
        /// Toggles the `--force` pull argument.
        ToggleForce,
        /// Pulls into the current branch from its upstream.
        PullFromUpstream,
    ]
);

/// Opens the pull transient in the minibuffer for the given Git panel.
pub fn open(
    workspace: &mut Workspace,
    panel: Entity<GitPanel>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let repository = workspace.project().read(cx).active_repository(cx);
    let branch = repository.and_then(|repo| repo.read(cx).branch.clone());
    let into_branch: SharedString = branch
        .as_ref()
        .map(|branch| branch.name().to_string().into())
        .unwrap_or_else(|| "HEAD".into());
    let upstream: SharedString = branch
        .as_ref()
        .and_then(|branch| branch.upstream.as_ref())
        .map(|upstream| {
            upstream
                .stripped_ref_name()
                .unwrap_or(upstream.ref_name.as_ref())
                .to_string()
                .into()
        })
        .unwrap_or_else(|| "no upstream configured".into());

    let popup = cx.new(|cx| PullPopup::new(panel.downgrade(), into_branch, upstream, cx));
    minibuffer::show(workspace, popup, window, cx);
}

pub struct PullPopup {
    panel: WeakEntity<GitPanel>,
    args: PullArgs,
    into_branch: SharedString,
    upstream: SharedString,
    focus_handle: FocusHandle,
}

impl PullPopup {
    fn new(
        panel: WeakEntity<GitPanel>,
        into_branch: SharedString,
        upstream: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            panel,
            args: PullArgs::default(),
            into_branch,
            upstream,
            focus_handle: cx.focus_handle(),
        }
    }

    fn toggle_ff_only(&mut self, _: &ToggleFfOnly, _: &mut Window, cx: &mut Context<Self>) {
        self.args.ff_only = !self.args.ff_only;
        cx.notify();
    }

    fn toggle_rebase(&mut self, _: &ToggleRebase, _: &mut Window, cx: &mut Context<Self>) {
        self.args.rebase = !self.args.rebase;
        cx.notify();
    }

    fn toggle_autostash(&mut self, _: &ToggleAutostash, _: &mut Window, cx: &mut Context<Self>) {
        self.args.autostash = !self.args.autostash;
        cx.notify();
    }

    fn toggle_force(&mut self, _: &ToggleForce, _: &mut Window, cx: &mut Context<Self>) {
        self.args.force = !self.args.force;
        cx.notify();
    }

    fn pull_from_upstream(
        &mut self,
        _: &PullFromUpstream,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let args = self.args;
        self.panel
            .update(cx, |panel, cx| panel.pull(args, window, cx))
            .ok();
        cx.emit(DismissEvent);
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl Focusable for PullPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for PullPopup {}

impl Render for PullPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("GitPullPopup")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_ff_only))
            .on_action(cx.listener(Self::toggle_rebase))
            .on_action(cx.listener(Self::toggle_autostash))
            .on_action(cx.listener(Self::toggle_force))
            .on_action(cx.listener(Self::pull_from_upstream))
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .p_3()
            .gap_1()
            .child(render_section("Arguments"))
            .child(render_argument(
                "-f",
                "Fast-forward only",
                "(--ff-only)",
                self.args.ff_only,
            ))
            .child(render_argument(
                "-r",
                "Rebase local commits",
                "(--rebase)",
                self.args.rebase,
            ))
            .child(render_argument(
                "-a",
                "Autostash",
                "(--autostash)",
                self.args.autostash,
            ))
            .child(render_argument("-F", "Force", "(--force)", self.args.force))
            .child(div().h_2())
            .child(
                h_flex()
                    .gap_1()
                    .child(Label::new("Pull into").color(Color::Muted))
                    .child(Label::new(self.into_branch.clone()).color(Color::Info))
                    .child(Label::new("from").color(Color::Muted)),
            )
            .child(render_action("p", self.upstream.clone()))
            .child(render_action("u", self.upstream.clone()))
    }
}
