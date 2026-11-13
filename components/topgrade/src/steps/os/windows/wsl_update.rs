use std::io::Write;
use std::path::Path;
use std::process::Output;

use color_eyre::eyre::Result;

use crate::error::TopgradeError;
use crate::execution_context::ExecutionContext;
use crate::executor::ExecutorOutput;
use crate::step::Step;
use crate::terminal::print_warning;
use crate::utils::which;

use super::winget::{self, UPDATE_NOT_APPLICABLE};

pub(super) fn run(ctx: &ExecutionContext, wsl: &Path) -> Result<()> {
    let pre_release = ctx.config().wsl_update_pre_release();
    let ExecutorOutput::Wet(output) = ctx
        .execute(wsl)
        .arg("--update")
        .arg_if(pre_release, "--pre-release")
        .arg_if(ctx.config().wsl_update_use_web_download(), "--web-download")
        .output()?
    else {
        return Ok(());
    };
    replay(&output)?;
    if output.status.success() {
        return Ok(());
    }
    if !can_use_stable_fallback(pre_release, &output) {
        return Err(process_error(&output, "wsl --update"));
    }
    let Some(winget) = which("winget")? else {
        return Err(process_error(&output, "wsl --update; WinGet fallback unavailable"));
    };
    print_warning(
        "The WSL update service rejected the HTTP request. Checking the exact Microsoft.WSL stable package through the WinGet community source instead.",
    );
    let mut command = if ctx.config().winget_use_sudo() {
        ctx.require_sudo()?.execute(ctx, &winget)?
    } else {
        ctx.execute(&winget)
    };
    let ExecutorOutput::Wet(fallback) = command
        .args([
            "upgrade",
            "--id",
            "Microsoft.WSL",
            "--exact",
            "--source",
            "winget",
            "--silent",
            "--disable-interactivity",
        ])
        .arg_if(ctx.config().yes(Step::WslUpdate), "--accept-package-agreements")
        .output()?
    else {
        return Ok(());
    };
    replay(&fallback)?;
    verify_fallback(&fallback)?;
    winget::verify_exact_after_upgrade(ctx, &winget, "Microsoft.WSL")
}

fn can_use_stable_fallback(pre_release: bool, output: &Output) -> bool {
    if pre_release || output.status.success() {
        return false;
    }
    // These HRESULT symbol names are invariant machine error codes, unlike
    // translated error prose. Do not mask permissions, install or other errors.
    [&output.stdout, &output.stderr].into_iter().any(|stream| {
        let text = decode_text(stream);
        text.contains("HTTP_E_STATUS_FORBIDDEN") || text.contains("HTTP_E_STATUS_TOO_MANY_REQUESTS")
    })
}

fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.contains(&0) {
        let words: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes)
            .collect();
        String::from_utf16_lossy(&words)
            .trim_start_matches('\u{feff}')
            .to_owned()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn replay(output: &Output) -> Result<()> {
    std::io::stdout()
        .lock()
        .write_all(decode_text(&output.stdout).as_bytes())?;
    std::io::stderr()
        .lock()
        .write_all(decode_text(&output.stderr).as_bytes())?;
    Ok(())
}

fn process_error(output: &Output, command: &str) -> color_eyre::eyre::Report {
    TopgradeError::ProcessFailedWithOutput(
        command.into(),
        output.status,
        format!("{}{}", decode_text(&output.stdout), decode_text(&output.stderr)),
    )
    .into()
}

fn verify_fallback(output: &Output) -> Result<()> {
    if output.status.success()
        || output
            .status
            .code()
            .is_some_and(|code| code as u32 == UPDATE_NOT_APPLICABLE)
    {
        Ok(())
    } else {
        Err(process_error(
            output,
            "winget upgrade --id Microsoft.WSL --exact --source winget",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn output(code: u32, stdout: Vec<u8>) -> Output {
        Output {
            status: ExitStatus::from_raw(code),
            stdout,
            stderr: Vec::new(),
        }
    }

    #[test]
    fn http_rejection_can_fallback_but_prerelease_and_other_errors_cannot() {
        for code in ["HTTP_E_STATUS_FORBIDDEN", "HTTP_E_STATUS_TOO_MANY_REQUESTS"] {
            let rejection = output(u32::MAX, format!("Wsl/UpdatePackage/{code}").into_bytes());
            assert!(can_use_stable_fallback(false, &rejection));
            assert!(!can_use_stable_fallback(true, &rejection));
        }
        assert!(!can_use_stable_fallback(false, &output(5, b"Access denied".to_vec())));
        assert!(!can_use_stable_fallback(
            false,
            &output(0, b"HTTP_E_STATUS_FORBIDDEN".to_vec())
        ));
    }

    #[test]
    fn utf16_wsl_error_codes_and_non_ascii_text_are_preserved() {
        let text = "更新失败: Wsl/UpdatePackage/HTTP_E_STATUS_FORBIDDEN\r\n";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_text(&bytes), text);
        assert!(can_use_stable_fallback(false, &output(u32::MAX, bytes)));
    }

    #[test]
    fn only_success_or_no_applicable_upgrade_completes_the_exact_fallback() {
        for code in [0, UPDATE_NOT_APPLICABLE] {
            assert!(verify_fallback(&output(code, Vec::new())).is_ok());
        }
        for code in [0x8A15_0014, 5] {
            assert!(verify_fallback(&output(code, b"diagnostic".to_vec())).is_err());
        }
    }
}
