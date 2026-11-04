use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt as _,
    process::Stdio,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use color_eyre::eyre::{Context, OptionExt, Result, ensure};

use crate::{
    command::CommandExt as _,
    executor::{Executor, ExecutorChild},
};

fn relay(reader: impl Read + Send + 'static, mut sink: impl Write + Send + 'static) -> mpsc::Receiver<Result<bool>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = (|| {
            let mut reader = BufReader::new(reader);
            let mut line = Vec::new();
            let mut failed = false;
            loop {
                let count = (&mut reader).take(65_537).read_until(b'\n', &mut line)?;
                if count == 0 {
                    return Ok(failed);
                }
                ensure!(count <= 65_536, "Cursor produced an oversized output line");
                sink.write_all(&line)?;
                sink.flush()?;
                failed |= line.trim_ascii_start().starts_with(b"Error while updating extension ");
                line.clear();
            }
        })();
        let _ = sender.send(result);
    });
    receiver
}

pub(super) fn run(command: &mut Executor, timeout: Duration) -> Result<()> {
    if let Executor::Wet(child) | Executor::Damp(child) = command {
        child.process_group(0).stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let ExecutorChild::Wet(mut child) = command.spawn_checked()? else {
        return Ok(());
    };
    let group = i32::try_from(child.id())?;
    let result = (|| {
        let stdout = relay(child.stdout.take().ok_or_eyre("Missing Cursor stdout")?, io::stdout());
        let stderr = relay(child.stderr.take().ok_or_eyre("Missing Cursor stderr")?, io::stderr());
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            ensure!(
                !crate::ctrlc::interrupted() && Instant::now() < deadline,
                "Cursor extension update interrupted or exceeded {} seconds",
                timeout.as_secs_f64()
            );
            thread::sleep(Duration::from_millis(50));
        };
        let drain = Instant::now() + Duration::from_secs(2);
        let out_failed = stdout.recv_timeout(drain.saturating_duration_since(Instant::now()))??;
        let err_failed = stderr.recv_timeout(drain.saturating_duration_since(Instant::now()))??;
        ensure!(status.success(), "Cursor extension update failed ({status})");
        ensure!(
            !out_failed && !err_failed,
            "Cursor reported an extension update failure"
        );
        Ok(())
    })();
    if let Err(error) = result {
        crate::studio::terminate_group(&mut child, group)
            .wrap_err_with(|| format!("{error:#}; unable to stop the Cursor updater"))?;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_context::RunType;

    #[test]
    fn rejects_cursor_failure_even_when_cli_exits_zero() {
        let mut command = RunType::Wet.execute("/bin/sh");
        command.args(["-c", "echo 'Error while updating extension example.test: aborted' >&2"]);
        let error = run(&mut command, Duration::from_secs(2)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Cursor reported an extension update failure")
        );
    }

    #[test]
    fn stops_a_stalled_cursor_process_group() {
        let mut command = RunType::Wet.execute("/bin/sh");
        command.args(["-c", "/bin/sleep 20 & wait"]);
        let error = run(&mut command, Duration::from_millis(50)).unwrap_err();
        assert!(error.to_string().contains("exceeded"));
        assert!(!error.to_string().contains("unable to stop"));
    }
}
