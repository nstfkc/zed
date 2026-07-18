use std::path::PathBuf;

use anyhow::{Context as _, Result};
use gpui::{App, Context, TaskExt as _, Window, actions};
use workspace::{OpenOptions, Workspace};

actions!(
    scratch_buffer,
    [
        /// Opens a persistent scratch buffer: a single global Markdown file for
        /// jotting notes, not bound to any project, whose contents persist
        /// across sessions and projects.
        OpenScratchBuffer
    ]
);

// This crate's object is kept linked by an explicit `black_box` reference to
// `init` in the zed binary's `main` — otherwise lld's --gc-sections drops the
// object and the `inventory` static that registers `OpenScratchBuffer`.
pub fn init(cx: &mut App) {
    cx.observe_new(register).detach();
}

fn register(workspace: &mut Workspace, _window: Option<&mut Window>, _: &mut Context<Workspace>) {
    workspace.register_action(|workspace, _: &OpenScratchBuffer, window, cx| {
        let abs_path = match scratch_buffer_path() {
            Ok(abs_path) => abs_path,
            Err(error) => {
                log::error!("failed to prepare scratch buffer file: {error:#}");
                return;
            }
        };
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
    });
}

/// Returns the absolute path to the global scratch buffer file, creating the
/// config directory and an empty file if they do not yet exist.
fn scratch_buffer_path() -> Result<PathBuf> {
    let config_dir = paths::config_dir();
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating config directory {config_dir:?}"))?;
    let path = config_dir.join("scratch.md");
    if !path.exists() {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("creating scratch buffer file {path:?}"))?;
    }
    Ok(path)
}
