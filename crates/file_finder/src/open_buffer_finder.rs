use std::sync::{
    Arc,
    atomic::{self, AtomicBool},
};

use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyContext, Render,
    Task, WeakEntity, Window, actions,
};
use picker::{Picker, PickerDelegate};
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};
use util::ResultExt;
use workspace::{ItemHandle, ModalView, Workspace};

actions!(
    file_finder,
    [
        /// Opens a fuzzy switcher over the buffers currently open in the
        /// workspace, letting you filter by name and jump to one.
        ToggleOpenBuffers
    ]
);

/// Registers the [`ToggleOpenBuffers`] action on the given workspace. Wired up
/// from `file_finder::init`.
pub fn register(
    workspace: &mut Workspace,
    _window: Option<&mut Window>,
    _: &mut Context<Workspace>,
) {
    workspace.register_action(|workspace, _: &ToggleOpenBuffers, window, cx| {
        let workspace_handle = cx.entity().downgrade();
        workspace.toggle_modal(window, cx, |window, cx| {
            OpenBufferFinder::new(workspace_handle, window, cx)
        });
    });
}

/// Thin modal wrapper around the picker, giving the finder its own key context.
pub struct OpenBufferFinder {
    picker: Entity<Picker<OpenBufferFinderDelegate>>,
    focus_handle: FocusHandle,
}

impl OpenBufferFinder {
    fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = OpenBufferFinderDelegate::new(workspace, cx);
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent))
            .detach();
        let focus_handle = picker.focus_handle(cx);
        Self {
            picker,
            focus_handle,
        }
    }
}

impl ModalView for OpenBufferFinder {}
impl EventEmitter<DismissEvent> for OpenBufferFinder {}

impl Focusable for OpenBufferFinder {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for OpenBufferFinder {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("OpenBufferFinder");
        v_flex().key_context(key_context).child(self.picker.clone())
    }
}

struct BufferEntry {
    item: Box<dyn ItemHandle>,
    candidate: StringMatchCandidate,
}

pub struct OpenBufferFinderDelegate {
    workspace: WeakEntity<Workspace>,
    entries: Vec<BufferEntry>,
    matches: Vec<StringMatch>,
    selected_index: usize,
    cancel_flag: Arc<AtomicBool>,
}

impl OpenBufferFinderDelegate {
    fn new(workspace: WeakEntity<Workspace>, cx: &mut Context<OpenBufferFinder>) -> Self {
        let entries = workspace
            .read_with(cx, |workspace, cx| collect_entries(workspace, cx))
            .unwrap_or_default();
        Self {
            workspace,
            entries,
            matches: Vec::new(),
            selected_index: 0,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Gathers the open items in the workspace as finder entries. The active item
/// is placed last so switching defaults to the most recently used other buffer.
fn collect_entries(workspace: &Workspace, cx: &App) -> Vec<BufferEntry> {
    let active_item_id = workspace.active_item(cx).map(|item| item.item_id());
    let mut entries: Vec<BufferEntry> = Vec::new();
    let mut active_entry: Option<BufferEntry> = None;
    for item in workspace.items(cx) {
        let title = item.tab_content_text(0, cx);
        let candidate = StringMatchCandidate::new(entries.len(), title.as_ref());
        let entry = BufferEntry {
            item: item.boxed_clone(),
            candidate,
        };
        if Some(item.item_id()) == active_item_id {
            active_entry = Some(entry);
        } else {
            entries.push(entry);
        }
    }
    if let Some(mut active_entry) = active_entry {
        active_entry.candidate.id = entries.len();
        entries.push(active_entry);
    }
    entries
}

impl PickerDelegate for OpenBufferFinderDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "open buffer finder"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        Arc::from("Switch to an open buffer…")
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, index: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.selected_index = index;
        cx.notify();
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.cancel_flag.store(true, atomic::Ordering::Release);
        self.cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = self.cancel_flag.clone();

        let candidates = self
            .entries
            .iter()
            .map(|entry| entry.candidate.clone())
            .collect::<Vec<_>>();

        cx.spawn(async move |this, cx| {
            let matches = fuzzy::match_strings(
                candidates.as_slice(),
                &query,
                false,
                true,
                100,
                &cancel_flag,
                cx.background_executor().clone(),
            )
            .await;
            if cancel_flag.load(atomic::Ordering::Acquire) {
                return;
            }
            this.update(cx, |this, cx| {
                this.delegate.matches = matches;
                this.delegate.selected_index = 0;
                cx.notify();
            })
            .ok();
        })
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(string_match) = self.matches.get(self.selected_index) else {
            return;
        };
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.candidate.id == string_match.candidate_id)
        else {
            return;
        };
        let item = entry.item.boxed_clone();
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.activate_item(item.as_ref(), true, true, window, cx);
            })
            .log_err();
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.cancel_flag.store(true, atomic::Ordering::Release);
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let string_match = self.matches.get(index)?;
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.candidate.id == string_match.candidate_id)?;
        Some(
            ListItem::new(index)
                .spacing(ListItemSpacing::Sparse)
                .inset(true)
                .toggle_state(selected)
                .child(HighlightedLabel::new(
                    entry.candidate.string.clone(),
                    string_match.positions.clone(),
                )),
        )
    }
}
