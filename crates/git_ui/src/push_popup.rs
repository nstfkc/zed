//! A magit-style transient popup for `git push`, rendered in the workspace's
//! minibuffer. It lists the push arguments as toggleable checkboxes (each bound
//! to its own key), shows the branch being pushed and where it will go, and runs
//! the push with the selected arguments.

use git::repository::PushArgs;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, WeakEntity, Window,
    actions,
};
use ui::prelude::*;
use workspace::Workspace;

use crate::git_panel::GitPanel;
use crate::transient::{render_action, render_argument, render_section};

actions!(
    git_push_popup,
    [
        /// Toggles the `--force-with-lease` push argument.
        ToggleForceWithLease,
        /// Toggles the `--force` push argument.
        ToggleForce,
        /// Toggles the `--no-verify` push argument.
        ToggleNoVerify,
        /// Toggles the `--dry-run` push argument.
        ToggleDryRun,
        /// Toggles the `--set-upstream` push argument.
        ToggleSetUpstream,
        /// Pushes the current branch to its upstream.
        PushToUpstream,
        /// Pushes the current branch to a remote chosen from a list.
        PushElsewhere,
    ]
);

/// Opens the push transient in the minibuffer for the given Git panel.
pub fn open(
    workspace: &mut Workspace,
    panel: Entity<GitPanel>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let repository = workspace.project().read(cx).active_repository(cx);
    let branch = repository.and_then(|repo| repo.read(cx).branch.clone());
    let from_branch: SharedString = branch
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

    let popup = cx.new(|cx| PushPopup::new(panel.downgrade(), from_branch, upstream, cx));
    minibuffer::show(workspace, popup, window, cx);
}

pub struct PushPopup {
    panel: WeakEntity<GitPanel>,
    args: PushArgs,
    force_with_lease: bool,
    from_branch: SharedString,
    upstream: SharedString,
    focus_handle: FocusHandle,
}

impl PushPopup {
    fn new(
        panel: WeakEntity<GitPanel>,
        from_branch: SharedString,
        upstream: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            panel,
            args: PushArgs::default(),
            force_with_lease: false,
            from_branch,
            upstream,
            focus_handle: cx.focus_handle(),
        }
    }

    fn toggle_force_with_lease(
        &mut self,
        _: &ToggleForceWithLease,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.force_with_lease = !self.force_with_lease;
        cx.notify();
    }

    fn toggle_force(&mut self, _: &ToggleForce, _: &mut Window, cx: &mut Context<Self>) {
        self.args.force = !self.args.force;
        cx.notify();
    }

    fn toggle_no_verify(&mut self, _: &ToggleNoVerify, _: &mut Window, cx: &mut Context<Self>) {
        self.args.no_verify = !self.args.no_verify;
        cx.notify();
    }

    fn toggle_dry_run(&mut self, _: &ToggleDryRun, _: &mut Window, cx: &mut Context<Self>) {
        self.args.dry_run = !self.args.dry_run;
        cx.notify();
    }

    fn toggle_set_upstream(
        &mut self,
        _: &ToggleSetUpstream,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.args.set_upstream = !self.args.set_upstream;
        cx.notify();
    }

    fn run_push(&mut self, select_remote: bool, window: &mut Window, cx: &mut Context<Self>) {
        let args = self.args;
        let force_with_lease = self.force_with_lease;
        self.panel
            .update(cx, |panel, cx| {
                panel.push_with_args(args, force_with_lease, select_remote, window, cx)
            })
            .ok();
        cx.emit(DismissEvent);
    }

    fn push_to_upstream(
        &mut self,
        _: &PushToUpstream,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_push(false, window, cx);
    }

    fn push_elsewhere(&mut self, _: &PushElsewhere, window: &mut Window, cx: &mut Context<Self>) {
        self.run_push(true, window, cx);
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl Focusable for PushPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for PushPopup {}

impl Render for PushPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("GitPushPopup")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_force_with_lease))
            .on_action(cx.listener(Self::toggle_force))
            .on_action(cx.listener(Self::toggle_no_verify))
            .on_action(cx.listener(Self::toggle_dry_run))
            .on_action(cx.listener(Self::toggle_set_upstream))
            .on_action(cx.listener(Self::push_to_upstream))
            .on_action(cx.listener(Self::push_elsewhere))
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .p_3()
            .gap_1()
            .child(render_section("Arguments"))
            .child(render_argument(
                "-f",
                "Force with lease",
                "(--force-with-lease)",
                self.force_with_lease,
            ))
            .child(render_argument("-F", "Force", "(--force)", self.args.force))
            .child(render_argument(
                "-h",
                "Disable hooks",
                "(--no-verify)",
                self.args.no_verify,
            ))
            .child(render_argument(
                "-n",
                "Dry run",
                "(--dry-run)",
                self.args.dry_run,
            ))
            .child(render_argument(
                "-u",
                "Set upstream",
                "(--set-upstream)",
                self.args.set_upstream,
            ))
            .child(div().h_2())
            .child(
                h_flex()
                    .gap_1()
                    .child(Label::new("Push").color(Color::Muted))
                    .child(Label::new(self.from_branch.clone()).color(Color::Info))
                    .child(Label::new("to").color(Color::Muted)),
            )
            .child(render_action("p", self.upstream.clone()))
            .child(render_action("e", "elsewhere"))
    }
}
