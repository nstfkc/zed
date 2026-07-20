//! Runs arbitrary shell commands from the minibuffer.
//!
//! `exec_command::Toggle` opens the minibuffer in single-line mode with a text
//! input. Confirming it starts the command through the user's shell, grows the
//! minibuffer to its full height and streams the combined stdout/stderr there.
//! Escape stops the command and closes the minibuffer; `d` leaves it running in
//! the background.
//!
//! Every run is kept in a session-wide history (see [`CommandHistory`]), so its
//! output can be reopened later with `exec_command::ToggleHistory`, whether the
//! command is still running or has already exited.

use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use editor::Editor;
use futures::channel::mpsc::{self, UnboundedSender};
use futures::{AsyncRead, AsyncReadExt as _, StreamExt as _};
use fuzzy::StringMatchCandidate;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Global,
    ScrollStrategy, Subscription, Task, UniformListScrollHandle, WeakEntity, Window, actions,
    uniform_list,
};
use picker::{Picker, PickerDelegate};
use ui::{ListItem, ListItemSpacing, prelude::*};
use util::ResultExt as _;
use util::command::Stdio;
use workspace::Workspace;

/// How much output a single run keeps. Older lines are dropped so a chatty
/// command (`yes`, a tailed log) can't grow without bound.
const MAX_OUTPUT_LINES: usize = 10_000;

actions!(
    exec_command,
    [
        /// Prompts for a shell command to run in the minibuffer.
        Toggle,
        /// Lists previously run shell commands so their output can be reopened.
        ToggleHistory,
        /// Closes the output view, leaving the command running in the background.
        Detach
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(toggle);
        workspace.register_action(toggle_history);
    })
    .detach();
}

fn toggle(workspace: &mut Workspace, _: &Toggle, window: &mut Window, cx: &mut Context<Workspace>) {
    let workspace_handle = cx.weak_entity();
    let view = cx.new(|cx| ExecCommand::prompt(workspace_handle, window, cx));
    minibuffer::show_with_options(workspace, view, true, window, cx);
}

fn toggle_history(
    workspace: &mut Workspace,
    _: &ToggleHistory,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let workspace_handle = cx.weak_entity();
    let view = cx.new(|cx| ExecCommand::history(workspace_handle, window, cx));
    minibuffer::show(workspace, view, window, cx);
}

/// Every command started in this window, most recent last. Runs stay here after
/// the minibuffer closes so their output can be reopened.
#[derive(Default)]
struct CommandHistory {
    runs: Vec<Entity<CommandRun>>,
}

impl Global for CommandHistory {}

#[derive(Clone, PartialEq)]
enum RunStatus {
    Running,
    Exited {
        code: Option<i32>,
    },
    /// The command was stopped from the output view.
    Stopped,
    /// The command could not be spawned or waited on.
    Failed(SharedString),
}

impl RunStatus {
    fn label(&self) -> SharedString {
        match self {
            RunStatus::Running => "running".into(),
            RunStatus::Exited { code: Some(0) } => "exited 0".into(),
            RunStatus::Exited { code: Some(code) } => format!("exited {code}").into(),
            RunStatus::Exited { code: None } => "terminated by signal".into(),
            RunStatus::Stopped => "stopped".into(),
            RunStatus::Failed(error) => format!("failed: {error}").into(),
        }
    }

    fn color(&self) -> Color {
        match self {
            RunStatus::Running => Color::Accent,
            RunStatus::Exited { code: Some(0) } => Color::Success,
            RunStatus::Exited { .. } | RunStatus::Stopped => Color::Warning,
            RunStatus::Failed(_) => Color::Error,
        }
    }
}

/// A single shell command and the output it has produced so far.
struct CommandRun {
    command: SharedString,
    lines: Vec<SharedString>,
    status: RunStatus,
    /// The child process is spawned with `kill_on_drop`, so dropping this task
    /// stops it. Detaching a run means simply leaving the task in place.
    task: Option<Task<()>>,
}

impl CommandRun {
    fn spawn(command: String, working_directory: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let (chunk_tx, mut chunk_rx) = mpsc::unbounded();
        let task = cx.spawn({
            let command = command.clone();
            async move |this, cx| {
                let child = cx.background_spawn(async move {
                    run_command(command, working_directory, chunk_tx).await
                });

                while let Some(chunk) = chunk_rx.next().await {
                    this.update(cx, |run, cx| {
                        run.push_output(&chunk);
                        cx.notify();
                    })
                    .ok();
                }

                let status = match child.await {
                    Ok(status) => RunStatus::Exited {
                        code: status.code(),
                    },
                    Err(error) => RunStatus::Failed(format!("{error:#}").into()),
                };
                this.update(cx, |run, cx| {
                    run.status = status;
                    cx.notify();
                })
                .ok();
            }
        });

        Self {
            command: command.into(),
            lines: Vec::new(),
            status: RunStatus::Running,
            task: Some(task),
        }
    }

    fn is_running(&self) -> bool {
        self.status == RunStatus::Running
    }

    /// Kills the child process by dropping the task that owns it.
    fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.is_running() {
            return;
        }
        self.task.take();
        self.status = RunStatus::Stopped;
        cx.notify();
    }

    fn push_output(&mut self, chunk: &str) {
        if self.lines.is_empty() {
            self.lines.push(SharedString::default());
        }
        for (ix, piece) in chunk.split('\n').enumerate() {
            if ix > 0 {
                self.lines.push(SharedString::default());
            }
            let piece = piece.trim_end_matches('\r');
            if piece.is_empty() {
                continue;
            }
            if let Some(last) = self.lines.last_mut() {
                let mut line = last.to_string();
                line.push_str(piece);
                *last = line.into();
            }
        }
        if self.lines.len() > MAX_OUTPUT_LINES {
            self.lines.drain(..self.lines.len() - MAX_OUTPUT_LINES);
        }
    }
}

async fn run_command(
    command: String,
    working_directory: Option<PathBuf>,
    chunk_tx: UnboundedSender<String>,
) -> Result<ExitStatus> {
    let shell = util::shell::get_system_shell();
    let mut process = util::command::new_command(&shell);
    process
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(working_directory) = working_directory {
        process.current_dir(working_directory);
    }

    let mut child = process
        .spawn()
        .with_context(|| format!("spawning `{shell} -c {command}`"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    futures::future::join(
        forward_output(stdout, chunk_tx.clone()),
        forward_output(stderr, chunk_tx),
    )
    .await;

    child
        .status()
        .await
        .with_context(|| format!("waiting for `{command}`"))
}

/// Streams a pipe to the run entity in whatever sized chunks arrive. A multi-byte
/// character split across two reads is rendered as replacement characters, which
/// is preferable to buffering output until the boundary resolves.
async fn forward_output(stream: Option<impl AsyncRead + Unpin>, chunk_tx: UnboundedSender<String>) {
    let Some(mut stream) = stream else {
        return;
    };
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => {
                let chunk = String::from_utf8_lossy(&buffer[..count]).into_owned();
                if chunk_tx.unbounded_send(chunk).is_err() {
                    break;
                }
            }
            Err(error) => {
                log::warn!("reading output of shell command failed: {error}");
                break;
            }
        }
    }
}

enum Mode {
    Prompt(Entity<Editor>),
    History {
        picker: Entity<Picker<CommandHistoryDelegate>>,
        _subscription: Subscription,
    },
    Output {
        run: Entity<CommandRun>,
        scroll_handle: UniformListScrollHandle,
        _observation: Subscription,
    },
}

/// The minibuffer content for running commands: the command prompt, the history
/// picker and the output view are all modes of a single view, so switching
/// between them keeps the same minibuffer panel (and its restore-focus target).
pub struct ExecCommand {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    mode: Mode,
}

impl ExecCommand {
    fn prompt(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Run shell command…", window, cx);
            editor
        });
        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            mode: Mode::Prompt(editor),
        }
    }

    fn history(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let runs = cx
            .try_global::<CommandHistory>()
            .map(|history| history.runs.iter().rev().cloned().collect())
            .unwrap_or_default();
        let delegate = CommandHistoryDelegate::new(cx.entity().downgrade(), runs);
        let picker = cx.new(|cx| {
            Picker::uniform_list(delegate, window, cx)
                .embedded()
                .full_width()
        });
        let subscription = cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| {
            cx.emit(DismissEvent);
        });
        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            mode: Mode::History {
                picker,
                _subscription: subscription,
            },
        }
    }

    fn show_output(
        &mut self,
        run: Entity<CommandRun>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scroll_handle = UniformListScrollHandle::new();
        let observation = cx.observe(&run, |this, run, cx| {
            if let Mode::Output { scroll_handle, .. } = &this.mode {
                let line_count = run.read(cx).lines.len();
                if line_count > 0 {
                    scroll_handle.scroll_to_item(line_count - 1, ScrollStrategy::Top);
                }
            }
            cx.notify();
        });
        self.mode = Mode::Output {
            run,
            scroll_handle,
            _observation: observation,
        };
        self.workspace
            .update(cx, |workspace, cx| {
                minibuffer::set_single_line(workspace, false, cx);
            })
            .ok();
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Mode::Prompt(editor) = &self.mode else {
            return;
        };
        let command = editor.read(cx).text(cx).trim().to_string();
        if command.is_empty() {
            return;
        }

        let working_directory = self
            .workspace
            .read_with(cx, |workspace, cx| {
                workspace.project().read(cx).first_project_directory(cx)
            })
            .ok()
            .flatten();
        let run = cx.new(|cx| CommandRun::spawn(command, working_directory, cx));
        cx.default_global::<CommandHistory>().runs.push(run.clone());
        self.show_output(run, window, cx);
    }

    /// Escape: stops a still-running command and closes the minibuffer.
    fn cancel(&mut self, _: &menu::Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        if let Mode::Output { run, .. } = &self.mode {
            run.update(cx, |run, cx| run.stop(cx));
        }
        cx.emit(DismissEvent);
    }

    /// `d`: closes the minibuffer but leaves the command running.
    fn detach(&mut self, _: &Detach, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn render_output(
        &self,
        run: &Entity<CommandRun>,
        scroll_handle: &UniformListScrollHandle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let weak_run = run.downgrade();
        let run = run.read(cx);
        let line_count = run.lines.len();
        let status = run.status.clone();
        let is_running = run.is_running();

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(Label::new(format!("$ {}", run.command)).truncate())
                    .child(
                        Label::new(status.label())
                            .size(LabelSize::Small)
                            .color(status.color()),
                    )
                    .when(is_running, |this| {
                        this.child(
                            Label::new("esc: stop · d: run in background")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    }),
            )
            .child(
                v_flex().size_full().px_2().py_1().child(
                    uniform_list(
                        "exec-command-output",
                        line_count,
                        move |range, _window, cx| {
                            let Some(run) = weak_run.upgrade() else {
                                return Vec::new();
                            };
                            run.read(cx)
                                .lines
                                .get(range)
                                .map(|lines| {
                                    lines
                                        .iter()
                                        .map(|line| {
                                            Label::new(line.clone())
                                                .buffer_font(cx)
                                                .size(LabelSize::Small)
                                                .single_line()
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        },
                    )
                    .track_scroll(scroll_handle)
                    .size_full(),
                ),
            )
    }
}

impl Render for ExecCommand {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let contents = match &self.mode {
            Mode::Prompt(editor) => h_flex()
                .size_full()
                .gap_1()
                .px_2()
                .child(Label::new("$").color(Color::Muted))
                .child(editor.clone())
                .into_any_element(),
            Mode::History { picker, .. } => picker.clone().into_any_element(),
            Mode::Output {
                run, scroll_handle, ..
            } => self
                .render_output(run, scroll_handle, cx)
                .into_any_element(),
        };

        div()
            .key_context(match self.mode {
                Mode::Prompt(_) => "ExecCommand",
                Mode::History { .. } => "ExecCommandHistory",
                Mode::Output { .. } => "ExecCommandOutput",
            })
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::detach))
            .size_full()
            .child(contents)
    }
}

impl Focusable for ExecCommand {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.mode {
            Mode::Prompt(editor) => editor.focus_handle(cx),
            Mode::History { picker, .. } => picker.focus_handle(cx),
            Mode::Output { .. } => self.focus_handle.clone(),
        }
    }
}

impl EventEmitter<DismissEvent> for ExecCommand {}

struct CommandHistoryDelegate {
    exec_command: WeakEntity<ExecCommand>,
    runs: Vec<Entity<CommandRun>>,
    matches: Vec<usize>,
    selected_index: usize,
}

impl CommandHistoryDelegate {
    fn new(exec_command: WeakEntity<ExecCommand>, runs: Vec<Entity<CommandRun>>) -> Self {
        Self {
            matches: (0..runs.len()).collect(),
            runs,
            exec_command,
            selected_index: 0,
        }
    }
}

impl PickerDelegate for CommandHistoryDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "shell command history"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Search previously run commands…".into()
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let candidates: Vec<_> = self
            .runs
            .iter()
            .enumerate()
            .map(|(ix, run)| StringMatchCandidate::new(ix, run.read(cx).command.as_ref()))
            .collect();
        if query.is_empty() {
            self.matches = (0..self.runs.len()).collect();
            self.selected_index = 0;
            return Task::ready(());
        }

        let executor = cx.background_executor().clone();
        let search = cx.background_executor().spawn(async move {
            fuzzy::match_strings(
                &candidates,
                &query,
                true,
                true,
                100,
                &Default::default(),
                executor,
            )
            .await
        });

        cx.spawn_in(window, async move |picker, cx| {
            let string_matches = search.await;
            picker
                .update(cx, |picker, cx| {
                    picker.delegate.matches = string_matches
                        .into_iter()
                        .map(|string_match| string_match.candidate_id)
                        .collect();
                    picker.delegate.selected_index = 0;
                    cx.notify();
                })
                .log_err();
        })
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(run) = self
            .matches
            .get(self.selected_index)
            .and_then(|ix| self.runs.get(*ix))
            .cloned()
        else {
            return;
        };
        self.exec_command
            .update(cx, |exec_command, cx| {
                exec_command.show_output(run, window, cx)
            })
            .ok();
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let run = self
            .matches
            .get(ix)
            .and_then(|ix| self.runs.get(*ix))?
            .read(cx);
        let status = run.status.clone();
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(Label::new(run.command.clone()).truncate())
                .end_slot(
                    Label::new(status.label())
                        .size(LabelSize::Small)
                        .color(status.color()),
                ),
        )
    }
}
