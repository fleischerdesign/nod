//! What an activation touched (ADR-029).
//!
//! `switch-to-configuration` names the units it restarted, started, reloaded or
//! added while it applies a generation. Those names are the only honest answer
//! to "what did this deployment change?", and they are what the post-activation
//! check has to verify first: a host can look calm while a unit the activation
//! started is already gone again.
//!
//! The same stream is the operator's live log, so nothing is buffered away:
//! every line is forwarded as it arrives, and only the marker lines are kept
//! for the parser. A watchdog that swallows the output it watches is useless
//! exactly when it is needed.

use std::collections::BTreeSet;
use std::process::{ExitStatus, Stdio};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Lines naming units the activation keeps or brings up.
///
/// A missing unit ends up in the snapshot through the probe, so a lie in this
/// list cannot hide a host state.
const ACTIVE_MARKERS: &[&str] = &[
    "restarting the following units:",
    "starting the following units:",
    "reloading the following units:",
    "the following new units were started:",
];

/// Unit suffixes systemd accepts. The filter keeps prose out of the set when a
/// message changes shape; a unit name always ends in one of these.
const UNIT_SUFFIXES: &[&str] = &[
    ".service",
    ".socket",
    ".timer",
    ".mount",
    ".automount",
    ".path",
    ".target",
    ".slice",
    ".scope",
    ".swap",
    ".device",
];

/// An activation that was watched while it ran.
pub struct Watched {
    /// The activation's own exit status.
    pub status: ExitStatus,
    /// The marker lines only, in arrival order, separated by newlines.
    pub markers: String,
}

/// Runs `command` with piped output, forwarding every line and keeping the
/// markers.
///
/// The two streams are pumped concurrently, so a chatty stderr cannot block a
/// child that writes to stdout. Bytes are forwarded unchanged (colours and
/// spacing survive), and non-UTF-8 output degrades to a lossy line instead of
/// aborting the read.
pub async fn watch(command: &mut Command) -> std::io::Result<Watched> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out = tokio::spawn(pump(stdout, Stream::Out));
    let err = tokio::spawn(pump(stderr, Stream::Err));

    let status = child.wait().await?;

    let mut markers = out.await.unwrap_or_default();
    markers.push_str(&err.await.unwrap_or_default());
    Ok(Watched { status, markers })
}

/// Which of the child's streams a pump forwards to.
enum Stream {
    Out,
    Err,
}

/// Forwards one stream line by line and returns the marker lines it saw.
async fn pump<R>(stream: Option<R>, stream_kind: Stream) -> String
where
    R: AsyncRead + Unpin,
{
    let Some(stream) = stream else {
        return String::new();
    };
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    let mut markers = String::new();

    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }

        match stream_kind {
            Stream::Out => {
                let mut sink = tokio::io::stdout();
                let _ = sink.write_all(&line).await;
                let _ = sink.flush().await;
            }
            Stream::Err => {
                let mut sink = tokio::io::stderr();
                let _ = sink.write_all(&line).await;
                let _ = sink.flush().await;
            }
        }

        let text = String::from_utf8_lossy(&line);
        keep_marker(text.trim_end_matches(['\n', '\r']), &mut markers);
    }

    markers
}

/// Appends `line` to `sink` when it names units that must end up active.
fn keep_marker(line: &str, sink: &mut String) {
    let plain = strip_ansi(line);
    if ACTIVE_MARKERS
        .iter()
        .any(|marker| plain.trim_start().starts_with(marker))
    {
        sink.push_str(line);
        sink.push('\n');
    }
}

/// Units the activation restarted, started, reloaded or added.
///
/// Deliberately *not* included: units the activation stopped, and changed units
/// it deliberately did not restart. A stop that went wrong surfaces as a failed
/// unit, and a unit stopped on purpose must never be required to run.
///
/// Names arrive space- or comma-separated depending on the NixOS generation, so
/// both separators are accepted. Anything that does not look like a unit is
/// dropped rather than scored.
pub fn touched_units(output: &str) -> BTreeSet<String> {
    let mut units = BTreeSet::new();

    for line in output.lines() {
        let plain = strip_ansi(line);
        let plain = plain.trim();

        for marker in ACTIVE_MARKERS {
            let Some(rest) = plain.strip_prefix(marker) else {
                continue;
            };
            for token in rest.split(|character: char| character.is_whitespace() || character == ',')
            {
                let token = token.trim_matches(['.', '"', '\'']);
                if UNIT_SUFFIXES.iter().any(|suffix| token.ends_with(suffix)) {
                    units.insert(token.to_string());
                }
            }
        }
    }

    units
}

/// Removes ANSI escape sequences so colour codes cannot break a prefix match.
///
/// `switch-to-configuration` colours its unit lists when it believes it talks
/// to a terminal; a captured stream may still carry the codes through `sudo`
/// or a wrapper, and matching on the raw text would silently stop working.
fn strip_ansi(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut characters = line.chars();

    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            plain.push(character);
            continue;
        }
        // CSI: ESC [ parameters letter. Anything else after ESC is one
        // character of payload and is skipped with it.
        if characters.next() == Some('[') {
            for character in characters.by_ref() {
                if character.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }

    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_active_marker_contributes_its_units() {
        let output = "\
restarting the following units: nginx.service caddy.service
starting the following units: app.socket
reloading the following units: sshd.service
the following new units were started: metrics.timer
";

        let units = touched_units(output);

        assert_eq!(
            units.into_iter().collect::<Vec<_>>(),
            vec![
                "app.socket",
                "caddy.service",
                "metrics.timer",
                "nginx.service",
                "sshd.service",
            ]
        );
    }

    #[test]
    fn units_separated_by_commas_are_split() {
        let units = touched_units("restarting the following units: a.service, b.service\n");
        assert_eq!(
            units.into_iter().collect::<Vec<_>>(),
            vec!["a.service", "b.service"]
        );
    }

    #[test]
    fn a_unit_named_twice_is_scored_once() {
        let output = "\
restarting the following units: web.service
starting the following units: web.service
";
        assert_eq!(touched_units(output).len(), 1);
    }

    #[test]
    fn stopped_and_deliberately_unchanged_units_are_not_touched() {
        let output = "\
stopping the following units: old.service
NOT restarting the following changed units: display-manager.service
";
        assert!(touched_units(output).is_empty());
    }

    #[test]
    fn coloured_output_still_matches() {
        let output =
            "\u{1b}[1;32mrestarting the following units:\u{1b}[0m \u{1b}[1mweb.service\u{1b}[0m\n";
        let units = touched_units(output);
        assert_eq!(units.into_iter().collect::<Vec<_>>(), vec!["web.service"]);
    }

    #[test]
    fn prose_after_a_marker_is_not_a_unit() {
        let units = touched_units("restarting the following units: and then it was done\n");
        assert!(units.is_empty());
    }

    #[test]
    fn unrelated_lines_contribute_nothing() {
        let units =
            touched_units("setting up tmpfiles\nwarning: something\nJob for web.service failed\n");
        assert!(units.is_empty());
    }

    #[tokio::test]
    async fn watch_forwards_and_keeps_only_the_markers() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "echo 'noise on stdout'; echo 'restarting the following units: web.service' >&2; exit 3",
        ]);

        let watched = watch(&mut command).await.expect("spawn should succeed");

        assert!(!watched.status.success());
        assert_eq!(
            touched_units(&watched.markers)
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["web.service"]
        );
    }

    #[tokio::test]
    async fn watch_reports_a_clean_exit() {
        let mut command = Command::new("sh");
        command.args(["-c", "true"]);
        let watched = watch(&mut command).await.expect("spawn should succeed");
        assert!(watched.status.success());
        assert!(touched_units(&watched.markers).is_empty());
    }
}
