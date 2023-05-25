//! Utilities for displaying task progress on the command line

use {
    std::{
        cmp::Ordering::*,
        fmt,
        io::{
            Stdout,
            stdout,
        },
        mem,
    },
    crossterm::terminal::{
        ClearType,
        disable_raw_mode,
        enable_raw_mode,
    },
    tokio::sync::{
        Mutex,
        broadcast,
    },
    crate::Task,
};

#[derive(Debug)]
struct State {
    lines: Vec<LineState>,
    selected_line: Option<LineId>,
    new_line_id: LineId,
    finalize_notifier: broadcast::Sender<()>,
    stdout: Stdout,
}

impl State {
    async fn update_line(&mut self, id: LineId) -> crossterm::Result<()> {
        let (self_idx, line) = self.lines.iter().enumerate().find(|(_, line)| line.id == id).expect("line not found");
        let selected_idx = self.selected_line.map_or_else(|| self.lines.len(), |selected_line| self.lines.iter().position(|line| line.id == selected_line).expect("line not found"));
        match self_idx.cmp(&selected_idx) {
            Less => {
                let line_diff = selected_idx - self_idx;
                crossterm::execute!(
                    self.stdout,
                    crossterm::cursor::MoveToPreviousLine(line_diff.try_into().expect("terminal too large")),
                    crossterm::style::Print(&line.text),
                    crossterm::terminal::Clear(ClearType::UntilNewLine),
                )?;
            }
            Equal => crossterm::execute!(
                self.stdout,
                crossterm::cursor::MoveToColumn(0),
                crossterm::style::Print(&line.text),
                crossterm::terminal::Clear(ClearType::UntilNewLine),
            )?,
            Greater => {
                let line_diff = self_idx - selected_idx;
                crossterm::execute!(
                    self.stdout,
                    crossterm::cursor::MoveToNextLine(line_diff.try_into().expect("terminal too large")),
                    crossterm::style::Print(&line.text),
                    crossterm::terminal::Clear(ClearType::UntilNewLine),
                )?;
            }
        }
        self.selected_line = Some(id);
        Ok(())
    }
}

/// A command-line progress renderer.
///
/// `Cli` does not implement [`Clone`]. If you need to share it across threads, consider wrapping it inside an [`Arc`](std::sync::Arc).
#[derive(Debug)]
pub struct Cli {
    state: Mutex<State>,
}

impl Cli {
    /// Returns a handle that allows coordinated line rendering.
    ///
    /// # Errors
    ///
    /// If the height of the terminal cannot be determined.
    pub fn new() -> crossterm::Result<Self> {
        enable_raw_mode()?;
        Ok(Self {
            state: Mutex::new(State {
                lines: Vec::default(),
                selected_line: None,
                new_line_id: LineId(0),
                finalize_notifier: broadcast::channel(1_024).0,
                stdout: stdout(),
            }),
        })
    }

    /// Waits until space is available at the bottom of the terminal, then creates a new line and returns a handle to it.
    ///
    /// # Correctness
    ///
    /// If `initial_text` is wider than the terminal or contains newlines or other control codes, the entire `Cli` may display incorrectly.
    pub async fn new_line<'a>(&'a self, initial_text: impl fmt::Display) -> crossterm::Result<LineHandle<'a>> {
        // make room for the line
        loop {
            let terminal_height = crossterm::terminal::size()?.1;
            let mut state = self.state.lock().await;
            if u16::try_from(state.lines.len()).expect("terminal too large") < terminal_height {
                // There is room on the terminal for a new line.
                break
            }
            if let Some(&LineState { finalized: true, id, .. }) = state.lines.get(0) {
                // There is a finalized line at the top of the CLI. Forget about this line, letting it scroll off the top of the screen.
                if state.selected_line == Some(id) {
                    if let Some(next_line) = state.lines.get(1) {
                        let next_id = next_line.id;
                        crossterm::execute!(
                            &mut state.stdout,
                            crossterm::cursor::MoveToNextLine(1),
                        )?;
                        state.selected_line = Some(next_id);
                    } else {
                        crossterm::execute!(
                            &mut state.stdout,
                            crossterm::style::Print("\r\n"),
                        )?;
                        state.selected_line = None;
                    }
                }
                state.lines.remove(0);
                continue
            }
            if let Some(idx) = state.lines.iter().position(|line| line.finalized) {
                // There is a finalized line below some unfinalized lines. Rearrange the lines to move the finalized line to the top so it can be forgotten about in the next iteration of the loop.
                let line = state.lines.remove(idx);
                state.lines.insert(0, line);
                for line_id in state.lines[..=idx].iter().map(|line| line.id).collect::<Vec<_>>() {
                    state.update_line(line_id).await?;
                }
                continue
            }
            // No room and no finalized lines. Wait until a line becomes finalized.
            let mut notifications = state.finalize_notifier.subscribe();
            drop(state);
            match notifications.recv().await {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => panic!("CLI notifier dropped"),
            }
            //TODO also listen for terminal resize events
        }
        let mut state = self.state.lock().await;
        // get an unused ID
        let mut id = state.new_line_id;
        state.new_line_id = LineId(state.new_line_id.0.wrapping_add(1));
        while state.lines.iter().any(|line| line.id == id) {
            id = state.new_line_id;
            state.new_line_id = LineId(state.new_line_id.0.wrapping_add(1));
        }
        // print the new line
        if let Some(selected_line) = state.selected_line {
            // Moves the cursor to the end of the lines managed by this value.
            let selected_idx = state.lines.iter().position(|line| line.id == selected_line).expect("line not found");
            let line_diff = state.lines.len() - 1 - selected_idx;
            crossterm::execute!(
                state.stdout,
                crossterm::cursor::MoveToNextLine(line_diff.try_into().expect("terminal too large")),
                crossterm::style::Print("\r\n"),
            )?;
            state.selected_line = None;
        }
        state.lines.push(LineState {
            finalized: false,
            text: initial_text.to_string(),
            id,
        });
        state.update_line(id).await?;
        Ok(LineHandle { id, cli: self })
    }

    /// Runs the given task to completion, displaying its progress in a new line below any existing lines.
    ///
    /// After the task is done, `done_label` is displayed as the final label of the task line. To have the label depend on the task's output, use [`Cli::run_with`].
    ///
    /// # Correctness
    ///
    /// The task's `Display` implementation is called each time the progress bar is updated. Returning text that's wider than the remainder of the terminal after the 7-columns-wide percentage indicator or contains newlines or other control codes may cause the entire `Cli` to display incorrectly. The same restriction applies to `done_label`.
    pub async fn run<T>(&self, task: impl Task<T> + fmt::Display, done_label: impl fmt::Display) -> crossterm::Result<T> {
        self.run_with(task, |_| done_label).await
    }

    /// Runs the given task to completion, displaying its progress in a new line below any existing lines.
    ///
    /// After the task is done, `done_label` is called with a reference to the task's output to display the final label of the task line.
    ///
    /// # Correctness
    ///
    /// The task's `Display` implementation is called each time the progress bar is updated. Returning text that's wider than the remainder of the terminal after the 7-columns-wide percentage indicator or contains newlines or other control codes may cause the entire `Cli` to display incorrectly. The same restriction applies to `done_label`.
    pub async fn run_with<T, A: Task<T> + fmt::Display, L: fmt::Display, F: FnOnce(&T) -> L>(&self, mut task: A, done_label: F) -> crossterm::Result<T> {
        let line = self.new_line(format!("[  0%] {task}")).await?;
        loop {
            match task.run().await {
                Ok(result) => {
                    line.replace(format!("[done] {}", done_label(&result))).await?;
                    line.drop_async().await;
                    break Ok(result)
                }
                Err(next_task) => {
                    task = next_task;
                    line.replace(format!("[{:>3}%] {task}", u8::from(task.progress()))).await?;
                }
            }
        }
    }

    /// Prevents this CLI from drawing to the terminal for the lifetime of the return value.
    ///
    /// This can be useful if you're spawning a subprocess that uses the alternate screen. If the subprocess in question writes to the primary screen, it will most likely cause the `Cli` to display incorrectly.
    pub async fn lock<'a>(&'a self) -> impl Send + Sync + 'a {
        self.state.lock().await
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        if let Some(selected_line) = state.selected_line {
            // Moves the cursor to the end of the lines managed by this value.
            let selected_idx = state.lines.iter().position(|line| line.id == selected_line).expect("line not found");
            let line_diff = state.lines.len() - 1 - selected_idx;
            let _ = crossterm::execute!(
                state.stdout,
                crossterm::cursor::MoveToNextLine(line_diff.try_into().expect("terminal too large")),
                crossterm::style::Print("\r\n"),
            );
        }
        let _ = disable_raw_mode();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineId(usize);

#[derive(Debug)]
struct LineState {
    id: LineId,
    finalized: bool,
    text: String,
}

/// A handle to a line.
///
/// As long as this value exists, the line it represents will be kept on screen and can be edited.
#[derive(Debug)]
pub struct LineHandle<'a> {
    cli: &'a Cli,
    id: LineId,
}

impl<'a> LineHandle<'a> {
    /// Replaces the contents of this line with the given text.
    ///
    /// # Correctness
    ///
    /// If `new_text` is wider than the terminal or contains newlines or other control codes, the entire `Cli` may display incorrectly.
    pub async fn replace(&self, new_text: impl fmt::Display) -> crossterm::Result<()> {
        let mut state = self.cli.state.lock().await;
        state.lines.iter_mut().find(|line| line.id == self.id).expect("line not found").text = new_text.to_string();
        state.update_line(self.id).await
    }

    /// Mark this line as finalized. It can no longer be edited, and may scroll off the top of the screen.
    ///
    /// Lines can only be added if there is room on the screen. If a new line is requested and there is no room, the topmost finalized line that's below an interactive line will be moved above all interactive lines.
    pub async fn drop_async(self) {
        let mut state = self.cli.state.lock().await;
        state.lines.iter_mut().find(|line| line.id == self.id).expect("line not found").finalized = true;
        let _ = state.finalize_notifier.send(());
        mem::forget(self);
    }
}

impl<'a> Drop for LineHandle<'a> {
    /// Mark this line as finalized. It can no longer be edited, and may scroll off the top of the screen.
    ///
    /// Lines can only be added if there is room on the screen. If a new line is requested and there is no room, the topmost finalized line that's below an interactive line will be moved above all interactive lines.
    ///
    /// # Panics
    ///
    /// If called from inside an async context. In that situation, `LineHandle::drop_async` should be used instead.
    fn drop(&mut self) {
        let mut state = self.cli.state.blocking_lock();
        state.lines.iter_mut().find(|line| line.id == self.id).expect("line not found").finalized = true;
        let _ = state.finalize_notifier.send(());
    }
}
