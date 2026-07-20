use gpui::{App, Context, Subscription, WeakEntity, Window, prelude::*};
use ui::{CommonAnimationExt, Label, LabelSize, prelude::*};
use workspace::{StatusItemView, Workspace, item::ItemHandle};

use crate::git_panel::{GitPanel, RemoteOperationStatus};

/// A status-bar item, shown centered, that surfaces the progress and result of
/// git push / pull / fetch operations triggered from the Git panel.
pub struct GitRemoteStatus {
    workspace: WeakEntity<Workspace>,
    panel: Option<WeakEntity<GitPanel>>,
    _git_store_subscription: Subscription,
    _panel_subscription: Option<Subscription>,
}

impl GitRemoteStatus {
    pub fn new(workspace: &Workspace, cx: &mut Context<Self>) -> Self {
        let git_store = workspace.project().read(cx).git_store().clone();
        // The Git panel is created after the status bar, so re-render on git
        // store activity and use those ticks to lazily hook up to the panel
        // once it exists.
        let _git_store_subscription = cx.observe(&git_store, |this, _, cx| {
            this.ensure_panel_subscription(cx);
            cx.notify();
        });
        Self {
            workspace: workspace.weak_handle(),
            panel: None,
            _git_store_subscription,
            _panel_subscription: None,
        }
    }

    fn ensure_panel_subscription(&mut self, cx: &mut Context<Self>) {
        if self._panel_subscription.is_some() {
            return;
        }
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(panel) = workspace.read(cx).panel::<GitPanel>(cx) else {
            return;
        };
        self.panel = Some(panel.downgrade());
        self._panel_subscription = Some(cx.observe(&panel, |_, _, cx| cx.notify()));
    }

    fn status(&self, cx: &App) -> Option<RemoteOperationStatus> {
        self.panel.as_ref()?.upgrade()?.read(cx).remote_operation_status()
    }
}

impl Render for GitRemoteStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex().when_some(self.status(cx), |this, status| match status {
            RemoteOperationStatus::InProgress(kind) => this
                .gap_1p5()
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
                ),
            RemoteOperationStatus::Finished(result) => {
                let (icon, color) = if result.is_error {
                    (IconName::XCircle, Color::Error)
                } else {
                    (IconName::Check, Color::Success)
                };
                this.gap_1p5()
                    .child(Icon::new(icon).size(IconSize::Small).color(color))
                    .child(
                        Label::new(result.message)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    )
            }
        })
    }
}

impl StatusItemView for GitRemoteStatus {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_panel_subscription(cx);
    }

    fn hide_setting(&self, _cx: &App) -> Option<workspace::HideStatusItem> {
        None
    }
}
