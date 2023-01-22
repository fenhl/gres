//! Utilities for displaying task progress on the command line

use {
    std::{
        cmp::Ordering::*,
        fmt,
        io::{
            Stdout,
            stdout,
        },
    },
    crossterm::terminal::ClearType,
    tokio::sync::{
        Mutex,
        Semaphore,
        SemaphorePermit,
    },
    crate::Task,
};

#[derive(Debug)]
struct State {
    num_lines: u16,
    current_line: u16,
    stdout: Stdout,
}

/// A command-line progress renderer.
///
/// `Cli` does not implement [`Clone`]. If you need to share it across threads, consider wrapping it inside an [`Arc`](std::sync::Arc).
#[derive(Debug)]
pub struct Cli {
    available_lines: Semaphore,
    state: Mutex<State>,
}

impl Cli {
    /// Returns a handle that allows coordinated line rendering.
    ///
    /// # Errors
    ///
    /// If the height of the terminal cannot be determined.
    pub fn new() -> crossterm::Result<Self> {
        Ok(Self {
            available_lines: Semaphore::new(crossterm::terminal::size()?.1.into()),
            state: Mutex::new(State {
                num_lines: 0,
                current_line: 0,
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
        let permit = self.available_lines.acquire().await.expect("line semaphore closed"); //TODO if no line is available immediately but there are finalized lines on screen, move the topmost one above any unfinalized lines (if necessary) and adjust state accordingly
        let mut state = self.state.lock().await;
        let line = state.num_lines;
        state.num_lines += 1;
        match line.cmp(&state.current_line) {
            Less => {
                let line_diff = state.current_line - line;
                crossterm::execute!(
                    state.stdout,
                    crossterm::cursor::MoveToPreviousLine(line_diff),
                    crossterm::style::Print(initial_text),
                )?;
            }
            Equal => crossterm::execute!(
                state.stdout,
                crossterm::cursor::MoveToColumn(0),
                crossterm::style::Print(initial_text),
            )?,
            Greater => {
                let line_diff = line - 1 - state.current_line;
                crossterm::execute!(
                    state.stdout,
                    crossterm::cursor::MoveToNextLine(line_diff),
                    crossterm::style::Print("\r\n"),
                    crossterm::style::Print(initial_text),
                )?;
            }
        }
        state.current_line = line;
        Ok(LineHandle { line, _permit: permit, cli: self })
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
        let line = self.new_line(format!("[  0%] {}", task)).await?;
        loop {
            match task.run().await {
                Ok(result) => {
                    line.replace(format!("[done] {}", done_label(&result))).await?;
                    break Ok(result)
                }
                Err(next_task) => {
                    task = next_task;
                    line.replace(format!("[{:>3}%] {}", u8::from(task.progress()), task)).await?;
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
    /// Moves the cursor to the end of the lines managed by this value.
    fn drop(&mut self) {
        let state = self.state.get_mut();
        if state.num_lines > 0 {
            let line_diff = state.num_lines - 1 - state.current_line;
            let _ = crossterm::execute!(
                state.stdout,
                crossterm::cursor::MoveToNextLine(line_diff),
                crossterm::style::Print('\n'),
            );
        }
    }
}

/// A handle to a line.
///
/// As long as this value exists, the line it represents will be kept on screen and can be edited.
#[derive(Debug)]
pub struct LineHandle<'a> {
    cli: &'a Cli,
    _permit: SemaphorePermit<'a>,
    line: u16, //TODO keep up to date when lines are rearranged (move to Cli.state?)
}

impl<'a> LineHandle<'a> {
    /// Replaces the contents of this line with the given text.
    ///
    /// # Correctness
    ///
    /// If `new_text` is wider than the terminal or contains newlines or other control codes, the entire `Cli` may display incorrectly.
    pub async fn replace(&self, new_text: impl fmt::Display) -> crossterm::Result<()> {
        let mut state = self.cli.state.lock().await;
        match self.line.cmp(&state.current_line) {
            Less => {
                let line_diff = state.current_line - self.line;
                crossterm::execute!(
                    state.stdout,
                    crossterm::cursor::MoveToPreviousLine(line_diff),
                    crossterm::style::Print(new_text),
                    crossterm::terminal::Clear(ClearType::UntilNewLine),
                )?;
            }
            Equal => crossterm::execute!(
                state.stdout,
                crossterm::cursor::MoveToColumn(0),
                crossterm::style::Print(new_text),
                crossterm::terminal::Clear(ClearType::UntilNewLine),
            )?,
            Greater => {
                let line_diff = self.line - state.current_line;
                crossterm::execute!(
                    state.stdout,
                    crossterm::cursor::MoveToNextLine(line_diff),
                    crossterm::style::Print(new_text),
                    crossterm::terminal::Clear(ClearType::UntilNewLine),
                )?;
            }
        }
        state.current_line = self.line;
        Ok(())
    }
}

impl<'a> Drop for LineHandle<'a> {
    /// Mark this line as finalized. It can no longer be edited, and may scroll off the top of the screen.
    ///
    /// Lines can only be added if there is room on the screen. If a new line is requested and there is no room, the topmost finalized line that's below an interactive line will be moved above all interactive lines.
    fn drop(&mut self) {
        //TODO mark this line as finalized
    }
}
