use gpui::{App, Context, Subscription, WeakEntity, Window, prelude::*};
use ui::{Label, LabelSize, prelude::*};
use workspace::{StatusItemView, Workspace, item::ItemHandle};

/// A status-bar item that shows the active repository's current branch name.
/// Useful when the title bar (which normally shows the branch) is hidden.
pub struct GitBranchStatus {
    workspace: WeakEntity<Workspace>,
    _subscription: Subscription,
}

impl GitBranchStatus {
    pub fn new(workspace: &Workspace, cx: &mut Context<Self>) -> Self {
        let git_store = workspace.project().read(cx).git_store().clone();
        // Re-render whenever the git store changes (branch switches, etc.).
        let _subscription = cx.observe(&git_store, |_, _, cx| cx.notify());
        Self {
            workspace: workspace.weak_handle(),
            _subscription,
        }
    }

    fn branch_name(&self, cx: &App) -> Option<SharedString> {
        let workspace = self.workspace.upgrade()?;
        let repository = workspace.read(cx).project().read(cx).active_repository(cx)?;
        let branch = repository.read(cx).branch.as_ref()?;
        Some(branch.name().to_string().into())
    }
}

impl Render for GitBranchStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex().when_some(self.branch_name(cx), |this, name| {
            this.gap_1()
                .child(
                    Icon::new(IconName::GitBranch)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(Label::new(name).size(LabelSize::Small).color(Color::Muted))
        })
    }
}

impl StatusItemView for GitBranchStatus {
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
