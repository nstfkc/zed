//! A magit-style transient popup for `git stash`, rendered in the workspace's
//! minibuffer. Each action operates on the whole working tree or on the most
//! recent stash entry; picking a specific entry is done from the stash list.

use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, WeakEntity, Window,
    actions,
};
use ui::prelude::*;
use util::ResultExt as _;
use workspace::Workspace;

use crate::git_panel::GitPanel;
use crate::transient::{render_action, render_section};

actions!(
    git_stash_popup,
    [
        /// Stashes all changes in the working tree.
        StashSave,
        /// Pops the most recent stash entry.
        StashPopLatest,
        /// Applies the most recent stash entry without dropping it.
        StashApplyLatest,
        /// Drops the most recent stash entry.
        StashDropLatest,
        /// Opens the list of stash entries.
        StashList,
    ]
);

/// Opens the stash transient in the minibuffer for the given Git panel.
pub fn open(
    workspace: &mut Workspace,
    panel: Entity<GitPanel>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let workspace_handle = workspace.weak_handle();
    let popup = cx.new(|cx| StashPopup::new(panel.downgrade(), workspace_handle, cx));
    minibuffer::show(workspace, popup, window, cx);
}

pub struct StashPopup {
    panel: WeakEntity<GitPanel>,
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
}

impl StashPopup {
    fn new(
        panel: WeakEntity<GitPanel>,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            panel,
            workspace,
            focus_handle: cx.focus_handle(),
        }
    }

    fn with_panel(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut GitPanel, &mut Context<GitPanel>),
    ) {
        self.panel.update(cx, f).ok();
        cx.emit(DismissEvent);
    }

    fn save(&mut self, _: &StashSave, window: &mut Window, cx: &mut Context<Self>) {
        self.with_panel(cx, |panel, cx| {
            panel.stash_all(&git::StashAll, window, cx);
        });
    }

    fn pop(&mut self, _: &StashPopLatest, window: &mut Window, cx: &mut Context<Self>) {
        self.with_panel(cx, |panel, cx| {
            panel.stash_pop(&git::StashPop, window, cx);
        });
    }

    fn apply(&mut self, _: &StashApplyLatest, window: &mut Window, cx: &mut Context<Self>) {
        self.with_panel(cx, |panel, cx| {
            panel.stash_apply(&git::StashApply, window, cx);
        });
    }

    fn drop_latest(&mut self, _: &StashDropLatest, _: &mut Window, cx: &mut Context<Self>) {
        self.with_panel(cx, |panel, cx| {
            panel.stash_drop_latest(cx);
        });
    }

    fn list(&mut self, _: &StashList, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        cx.emit(DismissEvent);
        // Deferred so the minibuffer has finished dismissing (and restoring
        // focus) before the stash list takes focus.
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    crate::stash_picker::open(workspace, &zed_actions::git::ViewStash, window, cx);
                })
                .log_err();
        });
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl Focusable for StashPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for StashPopup {}

impl Render for StashPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("GitStashPopup")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::pop))
            .on_action(cx.listener(Self::apply))
            .on_action(cx.listener(Self::drop_latest))
            .on_action(cx.listener(Self::list))
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .p_3()
            .gap_1()
            .child(render_section("Stash"))
            .child(render_action("z", "Save all changes"))
            .child(div().h_2())
            .child(render_section("Latest stash"))
            .child(render_action("p", "Pop"))
            .child(render_action("a", "Apply"))
            .child(render_action("k", "Drop"))
            .child(div().h_2())
            .child(render_action("l", "List stashes"))
    }
}
