use gpui::{App, Context, Subscription, WeakEntity, Window, prelude::*};
use project::git_store::{RemoteOperationKind, RemoteOperationResult};
use ui::{CommonAnimationExt, Label, LabelSize, prelude::*};
use workspace::{StatusItemView, Workspace, item::ItemHandle};

/// A status-bar item, shown centered, that surfaces the progress and result of
/// git push / pull / fetch operations. It observes the project's git store
/// directly, so it works regardless of whether a git panel is open.
pub struct GitRemoteStatus {
    workspace: WeakEntity<Workspace>,
    _subscription: Subscription,
}

impl GitRemoteStatus {
    pub fn new(workspace: &Workspace, cx: &mut Context<Self>) -> Self {
        let git_store = workspace.project().read(cx).git_store().clone();
        // Re-render whenever the git store changes (including when a remote
        // operation starts, finishes, or its result is cleared).
        let _subscription = cx.observe(&git_store, |_, _, cx| cx.notify());
        Self {
            workspace: workspace.weak_handle(),
            _subscription,
        }
    }

    fn in_progress(&self, cx: &App) -> Option<RemoteOperationKind> {
        let workspace = self.workspace.upgrade()?;
        let git_store = workspace.read(cx).project().read(cx).git_store().clone();
        git_store.read(cx).remote_operation()
    }

    fn result(&self, cx: &App) -> Option<RemoteOperationResult> {
        let workspace = self.workspace.upgrade()?;
        let git_store = workspace.read(cx).project().read(cx).git_store().clone();
        git_store.read(cx).remote_operation_result()
    }
}

impl Render for GitRemoteStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let element = h_flex().gap_1p5();
        if let Some(kind) = self.in_progress(cx) {
            element
                .child(
                    Icon::new(IconName::LoadCircle)
                        .size(IconSize::Small)
                        .color(Color::Muted)
                        .with_rotate_animation(2),
                )
                .child(
                    Label::new(format!("{}…", kind.in_progress_label()))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
        } else if let Some(result) = self.result(cx) {
            let (icon, color, message) = if result.is_error {
                (
                    IconName::XCircle,
                    Color::Error,
                    format!("{} failed", result.kind.name()),
                )
            } else {
                (
                    IconName::Check,
                    Color::Success,
                    result.kind.finished_label().to_string(),
                )
            };
            element
                .child(Icon::new(icon).size(IconSize::Small).color(color))
                .child(
                    Label::new(message)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
        } else {
            element
        }
    }
}

impl StatusItemView for GitRemoteStatus {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn hide_setting(&self, _cx: &App) -> Option<workspace::HideStatusItem> {
        None
    }
}
