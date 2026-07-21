use std::sync::Arc;

use fuzzy_nucleo::{StringMatch, StringMatchCandidate, match_strings};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, KeyContext, Render,
    Task, TaskExt, Window, actions,
};
use open_path_prompt::OpenPathDelegate;
use picker::{Picker, PickerDelegate};
use project::DirectoryLister;
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};
use workspace::{
    AppState, ModalView, OpenOptions, WorkspaceDb, WorkspaceId, with_active_or_new_workspace,
};

use crate::{
    RecentProjectEntry, RecentProjects, default_open_in_new_window, delete_recent_project,
    get_recent_projects, open_paths_as_project,
};

actions!(
    projectile,
    [
        /// Prompts for a folder to open as a project, adding it to the recent projects list.
        AddProject,
        /// Opens the recent projects fuzzy picker. Confirming opens the selected project.
        OpenProject,
        /// Opens a picker of recent projects. Confirming removes the selected entry from the list.
        DeleteProject,
        /// Returns to a clean home screen by opening a new empty window.
        Home,
    ]
);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &AddProject, cx| add_project(cx));
    cx.on_action(|_: &OpenProject, cx| open_project(cx));
    cx.on_action(|_: &DeleteProject, cx| delete_project(cx));
    cx.on_action(|_: &Home, cx| home(cx));
}

fn add_project(cx: &mut App) {
    with_active_or_new_workspace(cx, |workspace, window, cx| {
        let handle = cx.entity().downgrade();
        let create_new_window = default_open_in_new_window(cx);
        let lister = DirectoryLister::Local(
            workspace.project().clone(),
            workspace.app_state().fs.clone(),
        );
        // Pick the folder in the minibuffer rather than the OS file dialog. Tab
        // descends into a directory, Enter selects the highlighted one, and the
        // chosen path is then opened as a project.
        let (tx, rx) = futures::channel::oneshot::channel();
        let picker = cx.new(|cx| AddProjectPicker::new(lister, tx, window, cx));
        minibuffer::show(workspace, picker, window, cx);
        open_paths_as_project(handle, create_new_window, rx, window, cx);
    });
}

/// Minibuffer host for the folder picker used by [`AddProject`]. Wraps an
/// [`OpenPathDelegate`] picker (the same directory browser the open-path prompt
/// uses) so it can be shown in the minibuffer instead of a centered modal.
struct AddProjectPicker {
    picker: Entity<Picker<OpenPathDelegate>>,
    focus_handle: FocusHandle,
}

impl AddProjectPicker {
    fn new(
        lister: DirectoryLister,
        tx: futures::channel::oneshot::Sender<Option<Vec<std::path::PathBuf>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = OpenPathDelegate::new(tx, lister.clone(), false, cx).show_hidden();
        let picker =
            cx.new(|cx| Picker::uniform_list(delegate, window, cx).embedded().full_width());
        picker.update(cx, |picker, cx| {
            let query = lister.default_query(cx);
            picker.set_query(&query, window, cx);
        });
        cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent))
            .detach();
        let focus_handle = picker.focus_handle(cx);
        Self {
            picker,
            focus_handle,
        }
    }
}

impl ModalView for AddProjectPicker {}
impl EventEmitter<DismissEvent> for AddProjectPicker {}

impl Focusable for AddProjectPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AddProjectPicker {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("AddProjectPicker");
        v_flex().key_context(key_context).child(self.picker.clone())
    }
}

fn open_project(cx: &mut App) {
    with_active_or_new_workspace(cx, |workspace, window, cx| {
        let focus_handle = workspace.focus_handle(cx);
        // Default to opening in a new window so the current session's files are left untouched,
        // giving the user a clean session for the project they select.
        RecentProjects::open(
            workspace,
            Some(true),
            Vec::new(),
            window,
            focus_handle,
            cx,
        );
    });
}

fn delete_project(cx: &mut App) {
    with_active_or_new_workspace(cx, |workspace, window, cx| {
        let fs = workspace.app_state().fs.clone();
        let current_workspace_id = workspace.database_id();
        workspace.toggle_modal(window, cx, |window, cx| {
            DeleteProjectModal::new(fs, current_workspace_id, window, cx)
        });
    });
}

fn home(cx: &mut App) {
    let app_state = AppState::global(cx);
    workspace::open_new(
        OpenOptions::default(),
        app_state,
        cx,
        |_workspace, _window, _cx| {},
    )
    .detach_and_log_err(cx);
}

struct DeleteProjectModal {
    picker: Entity<Picker<DeleteProjectDelegate>>,
    focus_handle: FocusHandle,
}

impl DeleteProjectModal {
    fn new(
        fs: Arc<dyn fs::Fs>,
        current_workspace_id: Option<WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = DeleteProjectDelegate::default();
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent))
            .detach();
        let focus_handle = picker.focus_handle(cx);

        let db = WorkspaceDb::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            let projects = get_recent_projects(current_workspace_id, None, fs, &db).await;
            this.update_in(cx, |this, window, cx| {
                this.picker.update(cx, |picker, cx| {
                    picker.delegate.projects = projects;
                    let query = picker.query(cx);
                    picker.update_matches(query, window, cx);
                });
            })
            .ok();
        })
        .detach();

        Self {
            picker,
            focus_handle,
        }
    }
}

impl ModalView for DeleteProjectModal {}
impl EventEmitter<DismissEvent> for DeleteProjectModal {}

impl Focusable for DeleteProjectModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DeleteProjectModal {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("DeleteProjectModal");
        v_flex().key_context(key_context).child(self.picker.clone())
    }
}

#[derive(Default)]
struct DeleteProjectDelegate {
    projects: Vec<RecentProjectEntry>,
    matches: Vec<StringMatch>,
    selected_index: usize,
}

impl PickerDelegate for DeleteProjectDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "delete recent project"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        Arc::from("Remove a project from recent projects…")
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
        let query = query.trim();
        let case = fuzzy_nucleo::Case::smart_if_uppercase_in(query);
        let candidates: Vec<_> = self
            .projects
            .iter()
            .enumerate()
            .map(|(id, project)| StringMatchCandidate::new(id, project.name.as_ref()))
            .collect();

        let matches = match_strings(
            &candidates,
            query,
            case,
            fuzzy_nucleo::LengthPenalty::On,
            100,
        );

        self.matches = matches;
        self.selected_index = self
            .selected_index
            .min(self.matches.len().saturating_sub(1));
        cx.notify();
        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(selected_match) = self.matches.get(self.selected_index) else {
            return;
        };
        let Some(project) = self.projects.get(selected_match.candidate_id) else {
            return;
        };
        let workspace_id = project.workspace_id;
        let db = WorkspaceDb::global(cx);
        cx.spawn_in(window, async move |this, cx| {
            delete_recent_project(workspace_id, &db).await;
            this.update_in(cx, |picker, window, cx| {
                picker
                    .delegate
                    .projects
                    .retain(|project| project.workspace_id != workspace_id);
                let query = picker.query(cx);
                picker.update_matches(query, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some("No recent projects to remove".into())
    }

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let selected_match = self.matches.get(index)?;
        let project = self.projects.get(selected_match.candidate_id)?;
        Some(
            ListItem::new(index)
                .spacing(ListItemSpacing::Sparse)
                .inset(true)
                .toggle_state(selected)
                .child(
                    v_flex()
                        .min_w_0()
                        .child(HighlightedLabel::new(
                            project.name.to_string(),
                            selected_match.positions.clone(),
                        ))
                        .child(
                            Label::new(project.full_path.clone())
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                ),
        )
    }
}
