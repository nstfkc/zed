use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{self, AtomicBool},
    },
};

use file_icons::FileIcons;
use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle,
    KeyContext, Render, StyledText, Task, WeakEntity, Window, actions,
};
use picker::{Picker, PickerDelegate};
use project::{DirectoryItem, DirectoryLister};
use settings::Settings;
use ui::{HighlightedLabel, LabelLike, ListItem, ListItemSpacing, prelude::*};
use util::{
    ResultExt,
    paths::{PathStyle, compare_paths},
};
use workspace::{ModalView, OpenOptions, Workspace};

use crate::{file_finder_settings::FileFinderSettings, get_dir_and_suffix};

/// The name used for a parent-directory entry, and to navigate up a level.
const PARENT_DIR: &str = "..";

actions!(
    file_finder,
    [
        /// Opens a file finder scoped to the directory of the active editor's
        /// file. Lists that directory's entries, filters them as you type,
        /// descends into directories on confirm, and opens a new buffer for a
        /// typed name that matches no existing entry.
        FindFileInDirectory,
        /// Navigates the directory file finder up to the parent directory.
        GoToParentDirectory,
        /// Backspace in the directory finder: when the query is at a directory
        /// boundary (ends with a separator) removes the whole last folder,
        /// otherwise deletes a single character.
        DirectoryBackspace
    ]
);

/// Registers the [`FindFileInDirectory`] action on the given workspace. Wired
/// up from `file_finder::init`.
pub fn register(
    workspace: &mut Workspace,
    _window: Option<&mut Window>,
    _: &mut Context<Workspace>,
) {
    workspace.register_action(|workspace, _: &FindFileInDirectory, window, cx| {
        open(workspace, window, cx);
    });
}

fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let lister = DirectoryLister::Project(workspace.project().clone());
    let path_style = lister.path_style(cx);
    // The finder shows paths relative to the project root. `root` (absolute,
    // trailing separator) is prepended only when listing or opening a file.
    let root = lister.default_query(cx);
    // Start in the active file's directory, expressed relative to the root
    // (empty string means the root itself).
    let start_query = active_file_dir(workspace, path_style, cx)
        .and_then(|dir| dir.strip_prefix(&root).map(|relative| relative.to_string()))
        .unwrap_or_default();
    let workspace_handle = cx.entity().downgrade();
    let finder = cx.new(|cx| {
        DirectoryFinder::new(
            workspace_handle,
            lister,
            path_style,
            root,
            &start_query,
            window,
            cx,
        )
    });
    minibuffer::show(workspace, finder, window, cx);
}

/// Thin modal wrapper around the picker, giving the finder its own key context
/// so bindings like [`GoToParentDirectory`] can be scoped to it.
pub struct DirectoryFinder {
    picker: Entity<Picker<DirectoryFinderDelegate>>,
    focus_handle: FocusHandle,
}

impl DirectoryFinder {
    fn new(
        workspace: WeakEntity<Workspace>,
        lister: DirectoryLister,
        path_style: PathStyle,
        root: String,
        start_query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = DirectoryFinderDelegate::new(workspace, lister, path_style, root);
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx).embedded().full_width());
        picker.update(cx, |picker, cx| picker.set_query(start_query, window, cx));
        cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent))
            .detach();
        let focus_handle = picker.focus_handle(cx);
        Self {
            picker,
            focus_handle,
        }
    }

    fn go_to_parent_directory(
        &mut self,
        _: &GoToParentDirectory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let path_style = picker.delegate.path_style;
            let (dir, suffix) = get_dir_and_suffix(picker.query(cx), path_style);
            // If a filter is being typed, drop it back to the directory;
            // otherwise step up to the parent. Either way it's a single
            // keystroke that removes the whole trailing segment (with its
            // separator), rather than the separator and name separately.
            let target = if suffix.is_empty() {
                parent_dir(&dir, path_style)
            } else {
                Some(dir)
            };
            if let Some(target) = target {
                picker.set_query(&target, window, cx);
            }
        });
    }

    fn backspace(&mut self, _: &DirectoryBackspace, window: &mut Window, cx: &mut Context<Self>) {
        self.picker.update(cx, |picker, cx| {
            let path_style = picker.delegate.path_style;
            let query = picker.query(cx);
            let (dir, suffix) = get_dir_and_suffix(query.clone(), path_style);
            let new_query = if suffix.is_empty() {
                // At a directory boundary (query ends with a separator): remove
                // the whole last folder in a single keystroke.
                parent_dir(&dir, path_style).unwrap_or(dir)
            } else {
                // Mid-name: fall back to deleting one character.
                let mut query = query;
                query.pop();
                query
            };
            picker.set_query(&new_query, window, cx);
        });
    }
}

impl ModalView for DirectoryFinder {}
impl EventEmitter<DismissEvent> for DirectoryFinder {}

impl Focusable for DirectoryFinder {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DirectoryFinder {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("DirectoryFinder");
        v_flex()
            .key_context(key_context)
            .on_action(cx.listener(Self::go_to_parent_directory))
            .on_action(cx.listener(Self::backspace))
            .child(self.picker.clone())
    }
}

/// Absolute path (with a trailing separator) of the directory containing the
/// file open in the active editor, if any.
fn active_file_dir(workspace: &Workspace, path_style: PathStyle, cx: &App) -> Option<String> {
    let project = workspace.project().read(cx);
    let project_path = workspace.active_item(cx)?.project_path(cx)?;
    let abs_path = project
        .worktree_for_id(project_path.worktree_id, cx)?
        .read(cx)
        .absolutize(&project_path.path);
    let mut dir = abs_path.parent()?.to_string_lossy().into_owned();
    let separator = path_style.primary_separator();
    if !dir.ends_with(separator) {
        dir.push_str(separator);
    }
    Some(dir)
}

#[derive(Clone)]
struct Entry {
    candidate: StringMatchCandidate,
    is_dir: bool,
}

enum Selection {
    Existing { name: String, is_dir: bool },
    CreateNew { name: String },
}

pub struct DirectoryFinderDelegate {
    workspace: WeakEntity<Workspace>,
    lister: DirectoryLister,
    path_style: PathStyle,
    /// Absolute path (trailing separator) of the project root; prepended to the
    /// relative `parent_path` when listing directories or opening files.
    root: String,
    /// The directory `entries` were listed from, relative to `root` (empty for
    /// the root itself, otherwise trailing separator).
    parent_path: String,
    entries: Vec<Entry>,
    matches: Vec<StringMatch>,
    /// A name to offer creating a new file for, shown as the last row when the
    /// typed suffix matches no existing entry.
    create_name: Option<String>,
    selected_index: usize,
    cancel_flag: Arc<AtomicBool>,
}

impl DirectoryFinderDelegate {
    fn new(
        workspace: WeakEntity<Workspace>,
        lister: DirectoryLister,
        path_style: PathStyle,
        root: String,
    ) -> Self {
        Self {
            workspace,
            lister,
            path_style,
            root,
            parent_path: String::new(),
            entries: Vec::new(),
            matches: Vec::new(),
            create_name: None,
            selected_index: 0,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    fn selection(&self, index: usize) -> Option<Selection> {
        if index < self.matches.len() {
            let id = self.matches.get(index)?.candidate_id;
            let entry = self.entries.iter().find(|entry| entry.candidate.id == id)?;
            Some(Selection::Existing {
                name: entry.candidate.string.clone(),
                is_dir: entry.is_dir,
            })
        } else if index == self.matches.len() {
            self.create_name
                .clone()
                .map(|name| Selection::CreateNew { name })
        } else {
            None
        }
    }

    /// The directory to navigate to when a directory row is confirmed: the
    /// parent for `..`, otherwise the named child of the current directory.
    /// Includes a trailing separator.
    fn directory_target(&self, name: &str) -> Option<String> {
        if name == PARENT_DIR {
            parent_dir(&self.parent_path, self.path_style)
        } else {
            Some(format!(
                "{}{}{}",
                self.parent_path,
                name,
                self.path_style.primary_separator()
            ))
        }
    }
}

/// Builds the finder rows for `dir`: a leading `..` entry (unless `dir` is a
/// filesystem root), then the directory's children, directories first.
fn directory_entries(items: Vec<DirectoryItem>, dir: &str, path_style: PathStyle) -> Vec<Entry> {
    let mut items = items;
    items.sort_by(|a, b| compare_paths((&a.path, !a.is_dir), (&b.path, !b.is_dir)));
    let mut entries: Vec<Entry> = items
        .iter()
        .enumerate()
        .map(|(index, item)| Entry {
            candidate: StringMatchCandidate::new(index, &item.path.to_string_lossy()),
            is_dir: item.is_dir,
        })
        .collect();
    if parent_dir(dir, path_style).is_some() {
        let id = entries.len();
        entries.insert(
            0,
            Entry {
                candidate: StringMatchCandidate::new(id, PARENT_DIR),
                is_dir: true,
            },
        );
    }
    entries
}

/// The parent of a project-relative `path` (trailing separator), or `None` if
/// `path` is already the project root. The empty string is the root, so e.g.
/// `"apps/web/"` -> `"apps/"`, `"apps/"` -> `""`, and `""` -> `None`.
fn parent_dir(path: &str, _path_style: PathStyle) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let is_separator = |c: char| c == '/' || c == '\\';
    let trimmed = path.trim_end_matches(is_separator);
    match trimmed.rfind(is_separator) {
        Some(index) => Some(path[..=index].to_string()),
        None => Some(String::new()),
    }
}

impl PickerDelegate for DirectoryFinderDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "directory file finder"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        Arc::from("Search or create a file in this directory…")
    }

    fn match_count(&self) -> usize {
        self.matches.len() + usize::from(self.create_name.is_some())
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
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let path_style = self.path_style;
        let (dir, suffix) = get_dir_and_suffix(query, path_style);
        let list_task = (self.parent_path != dir)
            .then(|| self.lister.list_directory(format!("{}{}", self.root, dir), cx));

        self.cancel_flag.store(true, atomic::Ordering::Release);
        self.cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag = self.cancel_flag.clone();

        cx.spawn_in(window, async move |this, cx| {
            if let Some(list_task) = list_task {
                let entries =
                    directory_entries(list_task.await.log_err().unwrap_or_default(), &dir, path_style);
                if cancel_flag.load(atomic::Ordering::Acquire) {
                    return;
                }
                if this
                    .update(cx, |this, _| {
                        this.delegate.parent_path = dir.clone();
                        this.delegate.entries = entries;
                    })
                    .is_err()
                {
                    return;
                }
            }

            let Ok(candidates) = this.read_with(cx, |this, _| {
                this.delegate
                    .entries
                    .iter()
                    .map(|entry| entry.candidate.clone())
                    .collect::<Vec<_>>()
            }) else {
                return;
            };

            let matches = fuzzy::match_strings(
                candidates.as_slice(),
                &suffix,
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
                let has_exact_match = this
                    .delegate
                    .entries
                    .iter()
                    .any(|entry| entry.candidate.string == suffix);
                this.delegate.create_name = (!suffix.is_empty() && !has_exact_match).then_some(suffix);
                this.delegate.matches = matches;
                this.delegate.selected_index = 0;
                cx.notify();
            })
            .ok();
        })
    }

    fn confirm_update_query(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<String> {
        // Confirming a directory descends into it by rewriting the query,
        // rather than opening anything.
        match self.selection(self.selected_index)? {
            Selection::Existing {
                name,
                is_dir: true,
            } => self.directory_target(&name),
            _ => None,
        }
    }

    fn confirm_completion(
        &mut self,
        query: String,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<String> {
        // Tab completes the selected entry into the query.
        match self.selection(self.selected_index)? {
            Selection::Existing { name, is_dir: true } => {
                self.directory_target(&name).or(Some(query))
            }
            Selection::Existing { name, is_dir: false } => {
                Some(format!("{}{}", self.parent_path, name))
            }
            Selection::CreateNew { .. } => Some(query),
        }
    }

    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let name = match self.selection(self.selected_index) {
            // Directories are handled by `confirm_update_query`; guard anyway.
            Some(Selection::Existing { is_dir: true, .. }) | None => return,
            Some(Selection::Existing { name, .. }) | Some(Selection::CreateNew { name }) => name,
        };
        let abs_path = PathBuf::from(format!("{}{}{}", self.root, self.parent_path, name));

        self.workspace
            .update(cx, |workspace, cx| {
                if secondary {
                    workspace
                        .split_abs_path(abs_path, true, window, cx)
                        .detach_and_log_err(cx);
                } else {
                    workspace
                        .open_abs_path(
                            abs_path,
                            OpenOptions {
                                focus: Some(true),
                                ..Default::default()
                            },
                            window,
                            cx,
                        )
                        .detach_and_log_err(cx);
                }
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
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let show_file_icons = FileFinderSettings::get_global(cx).file_icons;
        let selection = self.selection(index)?;

        let item = match selection {
            Selection::Existing { name, is_dir } => {
                let icon = if show_file_icons {
                    let path = std::path::Path::new(&name);
                    let icon = if is_dir {
                        FileIcons::get_folder_icon(false, path, cx)
                    } else {
                        FileIcons::get_icon(path, cx)
                    };
                    icon.map(|icon| Icon::from_path(icon).color(Color::Muted))
                } else {
                    None
                };
                let positions = self
                    .matches
                    .get(index)
                    .map(|string_match| string_match.positions.clone())
                    .unwrap_or_default();
                ListItem::new(index)
                    .spacing(ListItemSpacing::Sparse)
                    .start_slot::<Icon>(icon)
                    .inset(true)
                    .toggle_state(selected)
                    .child(HighlightedLabel::new(name, positions))
            }
            Selection::CreateNew { name } => {
                let label = StyledText::new(format!("{name} (create)")).with_default_highlights(
                    &window.text_style(),
                    [(0..name.len(), HighlightStyle::color(Color::Created.color(cx)))],
                );
                ListItem::new(index)
                    .spacing(ListItemSpacing::Sparse)
                    .start_slot(Icon::new(IconName::Plus).color(Color::Muted))
                    .inset(true)
                    .toggle_state(selected)
                    .child(LabelLike::new().child(label))
            }
        };
        Some(item)
    }
}
