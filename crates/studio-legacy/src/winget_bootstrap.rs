use crate::utils::CancellationToken;
use std::path::{Path, PathBuf};

fn file_uri_string(path: &Path) -> anyhow::Result<String> {
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("MSIX 路径不是有效 Unicode"))?
        .replace('\\', "/");
    if !path.starts_with("//")
        && !(path.as_bytes().get(1) == Some(&b':') && path.as_bytes().get(2) == Some(&b'/'))
    {
        anyhow::bail!("MSIX 路径不是 Windows 绝对路径：{path}");
    }
    let mut uri = if path.starts_with("//") {
        String::from("file:")
    } else {
        String::from("file:///")
    };
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            uri.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(uri, "%{byte:02X}")?;
        }
    }
    Ok(uri)
}

#[cfg(target_os = "windows")]
pub fn install(
    bundle: &Path,
    dependencies: &[PathBuf],
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};
    use windows::Foundation::Uri;
    use windows::Management::Deployment::{DeploymentOptions, PackageManager};
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
    use windows::core::HSTRING;
    use windows_collections::IIterable;

    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { RoUninitialize() };
        }
    }

    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }?;
    let _apartment = Apartment;
    let uri =
        |path: &Path| Uri::CreateUri(&HSTRING::from(file_uri_string(path)?)).map_err(Into::into);
    let bundle_uri = uri(bundle)?;
    let dependency_uris = dependencies
        .iter()
        .map(|path| uri(path).map(Some))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let dependency_uris: IIterable<Uri> = dependency_uris.into();
    let operation = PackageManager::new()?.AddPackageAsync(
        &bundle_uri,
        &dependency_uris,
        DeploymentOptions::None,
    )?;
    let started = Instant::now();
    loop {
        if cancellation.is_cancelled() {
            let _ = operation.Cancel();
            anyhow::bail!("WinGet 安装已取消");
        }
        if started.elapsed() >= Duration::from_secs(600) {
            let _ = operation.Cancel();
            anyhow::bail!("WinGet 安装超过 10 分钟，已请求系统取消部署");
        }
        match operation.Status()?.0 {
            0 => std::thread::sleep(Duration::from_millis(250)),
            1 => {
                let result = operation.GetResults()?;
                let code = result.ExtendedErrorCode()?;
                if code.is_err() {
                    anyhow::bail!("MSIX 部署失败：{code:?} {}", result.ErrorText()?);
                }
                return Ok(());
            }
            2 => anyhow::bail!("WinGet 安装已被系统取消"),
            3 => anyhow::bail!("MSIX 部署失败：{:?}", operation.ErrorCode()?),
            status => anyhow::bail!("MSIX 部署返回未知状态：{status}"),
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn install(
    _bundle: &Path,
    _dependencies: &[PathBuf],
    _cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    anyhow::bail!("MSIX 部署仅支持 Windows")
}

#[cfg(test)]
mod tests {
    use super::file_uri_string;
    use std::path::Path;

    #[test]
    fn msix_uri_escapes_special_characters_and_preserves_unc_paths() {
        assert_eq!(
            file_uri_string(Path::new(r"C:\Users\我 #1\WinGet.msixbundle")).unwrap(),
            "file:///C:/Users/%E6%88%91%20%231/WinGet.msixbundle"
        );
        assert_eq!(
            file_uri_string(Path::new(r"\\host\share\WinGet.appx")).unwrap(),
            "file://host/share/WinGet.appx"
        );
        assert!(file_uri_string(Path::new("relative.appx")).is_err());
        assert!(file_uri_string(Path::new("C:relative.appx")).is_err());
    }
}
