use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{self, AtomicBool},
    },
};

use editor::Editor;
use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyContext, Render,
    Task, WeakEntity, Window, actions,
};
use language::Buffer;
use picker::{Picker, PickerDelegate};
use project::Location;
use text::{Bias, Point, ToPoint};
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};
use workspace::{ModalView, Workspace};

actions!(
    references,
    [
        /// Lists all references to the symbol under the cursor in a fuzzy
        /// picker; confirming opens the file at the referenced line.
        FindReferences
    ]
);

/// Registers the [`FindReferences`] action. Wired from `references_picker::init`.
pub fn init(cx: &mut App) {
    cx.observe_new(register).detach();
}

fn register(workspace: &mut Workspace, _: Option<&mut Window>, _: &mut Context<Workspace>) {
    workspace.register_action(|workspace, _: &FindReferences, window, cx| {
        open(workspace, window, cx);
    });
}

fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(editor) = workspace
        .active_item(cx)
        .and_then(|item| item.act_as::<Editor>(cx))
    else {
        return;
    };
    let project = workspace.project().clone();
    let anchor = editor.update(cx, |editor, cx| {
        let head = editor.selections.newest_anchor().head();
        editor.buffer().read(cx).text_anchor_for_position(head, cx)
    });
    let Some((buffer, position)) = anchor else {
        return;
    };
    let references = project.update(cx, |project, cx| project.references(&buffer, position, cx));

    cx.spawn_in(window, async move |workspace, cx| {
        let locations = references.await.ok().flatten().unwrap_or_default();
        let entries = cx
            .update(|_, cx| build_entries(&locations, cx))
            .unwrap_or_default();
        if entries.is_empty() {
            return;
        }
        let workspace_handle = workspace.clone();
        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.toggle_modal(window, cx, |window, cx| {
                    ReferencesPicker::new(workspace_handle, entries, window, cx)
                });
            })
            .ok();
    })
    .detach();
}

/// A single reference, with a precomputed display string and the location to
/// jump to on confirm.
#[derive(Clone)]
struct ReferenceEntry {
    buffer: Entity<Buffer>,
    point_range: Range<Point>,
    display: String,
}

fn build_entries(locations: &[Location], cx: &App) -> Vec<ReferenceEntry> {
    locations
        .iter()
        .map(|location| {
            let buffer = location.buffer.read(cx);
            let snapshot = buffer.text_snapshot();
            let start = location.range.start.to_point(&snapshot);
            let end = location.range.end.to_point(&snapshot);
            let row = start.row;
            let path = buffer
                .file()
                .map(|file| {
                    file.path()
                        .display(util::paths::PathStyle::local())
                        .to_string()
                })
                .unwrap_or_else(|| "untitled".to_string());
            let line_end = snapshot.clip_point(Point::new(row, u32::MAX), Bias::Left);
            let line_text: String = snapshot
                .text_for_range(Point::new(row, 0)..line_end)
                .collect();
            let display = format!("{}:{}  {}", path, row + 1, line_text.trim());
            ReferenceEntry {
                buffer: location.buffer.clone(),
                point_range: start..end,
                display,
            }
        })
        .collect()
}

pub struct ReferencesPicker {
    picker: Entity<Picker<ReferencesDelegate>>,
    focus_handle: FocusHandle,
}

impl ReferencesPicker {
    fn new(
        workspace: WeakEntity<Workspace>,
        entries: Vec<ReferenceEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = ReferencesDelegate::new(workspace, entries);
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

impl ModalView for ReferencesPicker {}
impl EventEmitter<DismissEvent> for ReferencesPicker {}

impl Focusable for ReferencesPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ReferencesPicker {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("ReferencesPicker");
        v_flex().key_context(key_context).child(self.picker.clone())
    }
}

pub struct ReferencesDelegate {
    workspace: WeakEntity<Workspace>,
    entries: Vec<ReferenceEntry>,
    matches: Vec<StringMatch>,
    selected_index: usize,
    cancel_flag: Arc<AtomicBool>,
}

impl ReferencesDelegate {
    fn new(workspace: WeakEntity<Workspace>, entries: Vec<ReferenceEntry>) -> Self {
        Self {
            workspace,
            entries,
            matches: Vec::new(),
            selected_index: 0,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl PickerDelegate for ReferencesDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "references picker"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        Arc::from("Filter references…")
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
            .enumerate()
            .map(|(index, entry)| StringMatchCandidate::new(index, &entry.display))
            .collect::<Vec<_>>();

        cx.spawn(async move |this, cx| {
            let matches = fuzzy::match_strings(
                candidates.as_slice(),
                &query,
                false,
                true,
                1000,
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
        let Some(entry) = self.entries.get(string_match.candidate_id) else {
            return;
        };
        let buffer = entry.buffer.clone();
        let range = entry.point_range.start..entry.point_range.start;
        self.workspace
            .update(cx, |workspace, cx| {
                let pane = workspace.active_pane().clone();
                let target_editor = workspace.open_project_item::<Editor>(
                    pane, buffer, true, true, false, false, window, cx,
                );
                target_editor.update(cx, |target_editor, cx| {
                    target_editor.go_to_singleton_buffer_range(range, window, cx);
                });
            })
            .ok();
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
        let entry = self.entries.get(string_match.candidate_id)?;
        Some(
            ListItem::new(index)
                .spacing(ListItemSpacing::Sparse)
                .inset(true)
                .toggle_state(selected)
                .child(HighlightedLabel::new(
                    entry.display.clone(),
                    string_match.positions.clone(),
                )),
        )
    }
}
