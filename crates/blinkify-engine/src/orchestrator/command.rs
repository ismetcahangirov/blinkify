//! A typed FFmpeg command line.
//!
//! `CLAUDE.md` forbidden behaviour 5: no command string, ever. This builder
//! goes further than "use an argument vector", because an argument vector
//! alone does not stop a filename from being read as an option or a URL:
//!
//! - A file called `-y.mp4` passed after `-i` is fine, but the same name as an
//!   output is parsed as the `-y` flag.
//! - A file called `concat:a.mp4|b.mp4`, or anything with a `scheme:` prefix,
//!   is opened through that protocol rather than as a file.
//!
//! So option **names** can only be `&'static str` — written in Blinkify's
//! source, never derived from a file — and every path is passed through the
//! `file:` protocol, which FFmpeg opens as a plain local file whatever the name
//! contains. Quotes, spaces, emoji and non-ASCII characters then need no
//! escaping at all, because nothing is ever parsed by a shell.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;
use std::time::Duration;

/// Which sidecar binary a command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
}

/// What the orchestrator does with the child's standard output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stdout {
    /// Parsed as `-progress pipe:1` key-value blocks. Set by
    /// [`SidecarCommand::report_progress`].
    Progress,
    /// Handed to the job's consumer as it arrives: decoded samples, frames,
    /// packet listings.
    Stream,
    /// Collected whole and returned when the process exits. For small outputs
    /// only — `ffprobe` JSON.
    Collect,
}

/// A complete, typed invocation of a sidecar binary.
#[derive(Clone, PartialEq, Eq)]
pub struct SidecarCommand {
    tool: Tool,
    args: Vec<OsString>,
    stdout: Stdout,
    /// The media duration progress is measured against, when reporting it.
    progress_total: Option<Duration>,
}

impl SidecarCommand {
    /// `ffmpeg`, quiet, never reading the console, never overwriting a file
    /// it was not told it may.
    #[must_use]
    pub fn ffmpeg() -> Self {
        Self::new(Tool::Ffmpeg).flags(&["-hide_banner", "-nostdin", "-n"])
    }

    /// `ffprobe`, printing errors only.
    #[must_use]
    pub fn ffprobe() -> Self {
        Self::new(Tool::Ffprobe).flags(&["-hide_banner"])
    }

    fn new(tool: Tool) -> Self {
        Self {
            tool,
            args: Vec::new(),
            stdout: Stdout::Collect,
            progress_total: None,
        }
    }

    /// A bare option such as `-vn` or `-show_streams`.
    #[must_use]
    pub fn flag(mut self, name: &'static str) -> Self {
        self.args.push(name.into());
        self
    }

    /// Several bare options, in order.
    #[must_use]
    pub fn flags(self, names: &[&'static str]) -> Self {
        names.iter().fold(self, |command, name| command.flag(name))
    }

    /// An option and its value: `-c:v copy`, `-read_intervals 10%+20`.
    ///
    /// The value may come from anywhere, because it is always passed as the
    /// argument *after* a name Blinkify chose; FFmpeg never parses it as an
    /// option. It must not be a path — use [`SidecarCommand::input`] or
    /// [`SidecarCommand::output_file`], which add the `file:` protocol.
    #[must_use]
    pub fn option(mut self, name: &'static str, value: impl Into<OsString>) -> Self {
        self.args.push(name.into());
        self.args.push(value.into());
        self
    }

    /// An input file, opened through the `file:` protocol.
    #[must_use]
    pub fn input(self, path: &Path) -> Self {
        self.option("-i", file_url(path))
    }

    /// Synthetic input from a `lavfi` graph — used by tests and by the encoder
    /// capability probe, never with user data.
    #[must_use]
    pub fn lavfi_input(self, graph: &str) -> Self {
        self.option("-f", "lavfi").option("-i", graph)
    }

    /// Write the output to a file, through the `file:` protocol.
    ///
    /// Commands start with `-n`, so an existing file is never overwritten:
    /// `CLAUDE.md` section 19 requires confirmation for that, and confirmation
    /// is not the sidecar's to give.
    #[must_use]
    pub fn output_file(mut self, path: &Path) -> Self {
        self.args.push(file_url(path));
        self
    }

    /// Write the output to standard output, to be consumed as it arrives.
    #[must_use]
    pub fn output_stdout(mut self) -> Self {
        self.args.push("pipe:1".into());
        self.stdout = Stdout::Stream;
        self
    }

    /// Discard the output — for analysis passes whose result is on stderr or
    /// in the progress stream.
    #[must_use]
    pub fn output_null(self) -> Self {
        let mut command = self.option("-f", "null");
        command.args.push("-".into());
        command
    }

    /// Stream standard output to the job's consumer. For `ffprobe` listings,
    /// which have no output argument.
    #[must_use]
    pub fn stream_stdout(mut self) -> Self {
        self.stdout = Stdout::Stream;
        self
    }

    /// Report machine-readable progress on standard output, measured against
    /// `total` — the duration of media the command will process.
    ///
    /// `-progress pipe:1` rather than scraping the human-readable status line
    /// on stderr, which changes between FFmpeg versions.
    #[must_use]
    pub fn report_progress(mut self, total: Duration) -> Self {
        // Placed before everything else: `-progress` is a global option and
        // must precede the first input.
        let mut args: Vec<OsString> = vec!["-progress".into(), "pipe:1".into(), "-nostats".into()];
        args.append(&mut self.args);
        self.args = args;
        self.stdout = Stdout::Progress;
        self.progress_total = Some(total);
        self
    }

    #[must_use]
    pub fn tool(&self) -> Tool {
        self.tool
    }

    #[must_use]
    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    #[must_use]
    pub fn stdout(&self) -> Stdout {
        self.stdout
    }

    #[must_use]
    pub fn progress_total(&self) -> Option<Duration> {
        self.progress_total
    }
}

/// The command as a person would read it in an error report. For display
/// only: nothing ever executes this string.
impl fmt::Display for SidecarCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.tool {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
        })?;
        for arg in &self.args {
            let arg = arg.to_string_lossy();
            if arg.is_empty() || arg.contains([' ', '"', '\'']) {
                write!(f, " \"{}\"", arg.replace('"', "\\\""))?;
            } else {
                write!(f, " {arg}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for SidecarCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SidecarCommand({self})")
    }
}

/// `file:` plus the path, unchanged. FFmpeg strips the prefix and opens the
/// rest as a local file, converting UTF-8 to the wide Windows API and adding
/// the `\\?\` prefix for long paths itself.
fn file_url(path: &Path) -> OsString {
    let mut url = OsString::from("file:");
    url.push(OsStr::new(path));
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &SidecarCommand) -> Vec<String> {
        command
            .args()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_path_that_looks_like_an_option_or_a_url_stays_a_file() {
        let command = SidecarCommand::ffmpeg()
            .input(Path::new("-y.mp4"))
            .output_file(Path::new("concat:a.mp4|b.mp4"));
        let args = args(&command);
        assert!(args.contains(&"file:-y.mp4".to_owned()));
        assert_eq!(
            args.last().map(String::as_str),
            Some("file:concat:a.mp4|b.mp4")
        );
    }

    #[test]
    fn a_filename_with_quotes_and_emoji_is_one_argument_unescaped() {
        let name = r#"C:\Users\Işıq\it's "fine" 🎬.mp4"#;
        let command = SidecarCommand::ffprobe().input(Path::new(name));
        assert!(args(&command).contains(&format!("file:{name}")));
    }

    #[test]
    fn progress_is_requested_before_the_first_input() {
        let command = SidecarCommand::ffmpeg()
            .input(Path::new("in.mp4"))
            .output_null()
            .report_progress(Duration::from_secs(10));
        let args = args(&command);
        let progress = args.iter().position(|a| a == "-progress");
        let input = args.iter().position(|a| a == "-i");
        assert!(progress < input);
        assert_eq!(command.stdout(), Stdout::Progress);
    }

    #[test]
    fn ffmpeg_never_overwrites_by_default() {
        assert!(args(&SidecarCommand::ffmpeg()).contains(&"-n".to_owned()));
    }
}
