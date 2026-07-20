//! Runs arbitrary shell commands from the minibuffer.
//!
//! `exec_command::Toggle` opens the minibuffer in single-line mode with a text
//! input. Confirming it starts the command through the user's shell, grows the
//! minibuffer to its full height and streams the combined stdout/stderr there.
//! Escape stops the command and closes the minibuffer; `d` leaves it running in
//! the background.
//!
//! Runs are kept in [`CommandHistory`], so their output can be reopened later
//! with `exec_command::ToggleHistory`, whether the command is still running or
//! has already exited.
//!
//! Output is shown as plain text: neither ANSI escape sequences nor in-line
//! carriage returns (progress bars that redraw a line with `\r`) are
//! interpreted, so both show up literally.

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
use util::shell::ShellKind;
use workspace::Workspace;

/// How much output a single run keeps. Older lines are dropped so a chatty
/// command (`yes`, a tailed log) can't grow without bound.
const MAX_OUTPUT_LINES: usize = 10_000;

/// How many runs the history keeps. Finished runs past this are dropped, oldest
/// first, so a long session doesn't hold on to every command it ever ran.
const MAX_HISTORY_ENTRIES: usize = 100;

actions!(
    exec_command,
    [
        /// Prompts for a shell command to run in the minibuffer, or closes the
        /// prompt if it is already open.
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
    // Closing by emitting the dismiss event (rather than clearing the panel)
    // goes through the minibuffer's own teardown, which restores focus. A
    // running command is left running, same as detaching from it.
    if let Some(shown) = minibuffer::shown_content::<ExecCommand>(workspace, cx) {
        shown.update(cx, |_, cx| cx.emit(DismissEvent));
        return;
    }

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

/// Commands started in this Zed instance, most recent last. Runs stay here after
/// the minibuffer closes so their output can be reopened. The history is
/// app-wide, so every window sees the same runs.
#[derive(Default)]
struct CommandHistory {
    runs: Vec<Entity<CommandRun>>,
}

impl Global for CommandHistory {}

impl CommandHistory {
    fn push(&mut self, run: Entity<CommandRun>, cx: &App) {
        self.runs.push(run);
        // Only finished runs are evicted: dropping a running one would kill it,
        // which is the opposite of what detaching it asked for.
        while self.runs.len() > MAX_HISTORY_ENTRIES {
            let Some(oldest_finished) = self.runs.iter().position(|run| !run.read(cx).is_running())
            else {
                break;
            };
            self.runs.remove(oldest_finished);
        }
    }
}

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

/// The output of a run, split into lines for rendering. The line currently
/// being written is kept as an owned `String` so appending to it doesn't rebuild
/// it: a command that writes a long line in many small chunks (an unbuffered
/// progress report) would otherwise be quadratic in the line's length.
#[derive(Default)]
struct OutputLines {
    complete: Vec<SharedString>,
    partial: String,
}

impl OutputLines {
    fn push(&mut self, chunk: &str) {
        for (ix, piece) in chunk.split('\n').enumerate() {
            if ix > 0 {
                let line = std::mem::take(&mut self.partial);
                self.complete.push(line.into());
            }
            self.partial.push_str(piece.trim_end_matches('\r'));
        }
        if self.complete.len() > MAX_OUTPUT_LINES {
            self.complete
                .drain(..self.complete.len() - MAX_OUTPUT_LINES);
        }
    }

    fn len(&self) -> usize {
        self.complete.len() + if self.partial.is_empty() { 0 } else { 1 }
    }

    fn line(&self, ix: usize) -> Option<SharedString> {
        match self.complete.get(ix) {
            Some(line) => Some(line.clone()),
            None if ix == self.complete.len() && !self.partial.is_empty() => {
                Some(self.partial.clone().into())
            }
            None => None,
        }
    }
}

/// A single shell command and the output it has produced so far.
struct CommandRun {
    command: SharedString,
    output: OutputLines,
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
                        run.output.push(&chunk);
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
            output: OutputLines::default(),
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
}

async fn run_command(
    command: String,
    working_directory: Option<PathBuf>,
    chunk_tx: UnboundedSender<String>,
) -> Result<ExitStatus> {
    let shell = util::shell::get_system_shell();
    // Which flag introduces a command line depends on the shell: `-c` for POSIX
    // shells, `-C` for PowerShell (the Windows default), `/S /C` for cmd.
    let arguments = ShellKind::new(&shell, cfg!(windows)).args_for_shell(false, command.clone());
    let mut process = util::command::new_command(&shell);
    process
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(working_directory) = working_directory {
        process.current_dir(working_directory);
    }

    let mut child = process
        .spawn()
        .with_context(|| format!("spawning `{command}` with {shell}"))?;
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
                let line_count = run.read(cx).output.len();
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
        cx.update_default_global::<CommandHistory, _>(|history, cx| {
            history.push(run.clone(), cx);
        });
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
        let line_count = run.output.len();
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
                            let run = run.read(cx);
                            range
                                .filter_map(|ix| run.output.line(ix))
                                .map(|line| {
                                    Label::new(line)
                                        .buffer_font(cx)
                                        .size(LabelSize::Small)
                                        .single_line()
                                })
                                .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(output: &OutputLines) -> Vec<String> {
        (0..output.len())
            .filter_map(|ix| output.line(ix))
            .map(|line| line.to_string())
            .collect()
    }

    #[test]
    fn test_output_lines_splits_on_newlines() {
        let mut output = OutputLines::default();
        output.push("first\nsecond\n");
        assert_eq!(lines(&output), vec!["first", "second"]);

        output.push("third");
        assert_eq!(lines(&output), vec!["first", "second", "third"]);
    }

    #[test]
    fn test_output_lines_joins_partial_chunks() {
        let mut output = OutputLines::default();
        output.push("par");
        output.push("tial");
        assert_eq!(lines(&output), vec!["partial"]);

        output.push(" line\nnext");
        assert_eq!(lines(&output), vec!["partial line", "next"]);
    }

    #[test]
    fn test_output_lines_keeps_blank_lines_and_strips_carriage_returns() {
        let mut output = OutputLines::default();
        output.push("one\r\n\r\ntwo\r\n");
        assert_eq!(lines(&output), vec!["one", "", "two"]);
    }

    #[test]
    fn test_output_lines_keeps_embedded_carriage_returns_literal() {
        // Documents current behavior: a mid-line carriage return (progress bars)
        // is kept literally rather than overwriting the line.
        let mut output = OutputLines::default();
        output.push("50%\r100%\n");
        assert_eq!(lines(&output), vec!["50%\r100%"]);
    }

    #[test]
    fn test_output_lines_is_empty_before_any_output() {
        let output = OutputLines::default();
        assert_eq!(output.len(), 0);
        assert_eq!(output.line(0), None);
    }

    #[test]
    fn test_output_lines_drops_oldest_lines_past_the_cap() {
        let mut output = OutputLines::default();
        for line in 0..MAX_OUTPUT_LINES + 10 {
            output.push(&format!("line {line}\n"));
        }
        let lines = lines(&output);
        assert_eq!(lines.len(), MAX_OUTPUT_LINES);
        assert_eq!(lines.first().map(String::as_str), Some("line 10"));
        assert_eq!(
            lines.last().map(String::as_str),
            Some(format!("line {}", MAX_OUTPUT_LINES + 9).as_str())
        );
    }
}
