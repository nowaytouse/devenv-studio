use std::{mem::size_of, os::windows::ffi::OsStrExt, path::Path};

use color_eyre::eyre::{Result, bail, eyre};
use windows::{
    Win32::{
        Foundation::HWND,
        Security::{
            Cryptography::{CERT_NAME_ATTR_TYPE, CertGetNameStringW, szOID_ORGANIZATION_NAME},
            WinTrust::{
                WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
                WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
                WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData,
                WinVerifyTrust,
            },
        },
    },
    core::PCWSTR,
};

// The provider owns its signer and certificate pointers until the state is closed.
struct TrustState(WINTRUST_DATA);

impl Drop for TrustState {
    fn drop(&mut self) {
        self.0.dwStateAction = WTD_STATEACTION_CLOSE;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        unsafe {
            WinVerifyTrust(
                HWND(-1isize as _),
                &mut action,
                (&mut self.0 as *mut WINTRUST_DATA).cast(),
            )
        };
    }
}

fn verify(path: &Path, require_microsoft: bool) -> Result<()> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        bail!("Signature path contains a NUL character");
    }
    wide.push(0);
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        ..Default::default()
    };
    let mut state = TrustState(WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        ..Default::default()
    });
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = unsafe {
        WinVerifyTrust(
            HWND(-1isize as _),
            &mut action,
            (&mut state.0 as *mut WINTRUST_DATA).cast(),
        )
    };
    if status != 0 {
        let kind = match status as u32 {
            0x800b0100 => "Unsigned",
            0x80096010 | 0x800b0109 | 0x800b010a | 0x800b010c | 0x800b0111 | 0x800b0112 => "Invalid or untrusted",
            _ => "Verification error",
        };
        bail!("{kind} (Authenticode 0x{:08X})", status as u32);
    }
    if !require_microsoft {
        return Ok(());
    }
    unsafe {
        let provider = WTHelperProvDataFromStateData(state.0.hWVTStateData);
        if provider.is_null() {
            bail!("Authenticode provider state is unavailable");
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
        if signer.is_null() {
            bail!("Authenticode signer is unavailable");
        }
        let cert = WTHelperGetProvCertFromChain(signer, 0);
        if cert.is_null() || (*cert).pCert.is_null() {
            bail!("Authenticode signer certificate is unavailable");
        }
        let oid = Some(szOID_ORGANIZATION_NAME.0.cast());
        let len = CertGetNameStringW((*cert).pCert, CERT_NAME_ATTR_TYPE, 0, oid, None);
        if len <= 1 {
            bail!("Signer certificate has no organization name");
        }
        let mut name = vec![0u16; len as usize];
        let written = CertGetNameStringW((*cert).pCert, CERT_NAME_ATTR_TYPE, 0, oid, Some(&mut name));
        if written != len || name.last() != Some(&0) {
            bail!("Could not read signer certificate organization");
        }
        let organization = String::from_utf16(&name[..name.len() - 1])
            .map_err(|_| eyre!("Invalid signer certificate organization"))?;
        if !is_microsoft_organization(&organization) {
            bail!("Signer organization is {organization:?}, expected Microsoft Corporation");
        }
    }
    Ok(())
}

fn is_microsoft_organization(organization: &str) -> bool {
    organization == "Microsoft Corporation"
}

pub(super) fn verify_microsoft(path: &Path) -> Result<()> {
    verify(path, true)
}

pub(super) fn signature_status(path: &Path) -> String {
    match verify(path, false) {
        Ok(()) => "Valid".to_owned(),
        Err(err) => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn organization_must_match_exactly() {
        assert!(is_microsoft_organization("Microsoft Corporation"));
        assert!(!is_microsoft_organization("Microsoft Corporation Evil"));
        assert!(!is_microsoft_organization("microsoft corporation"));
    }

    #[test]
    fn rejects_unsigned_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"unsigned").unwrap();
        assert!(verify_microsoft(file.path()).is_err());
        assert_ne!(signature_status(file.path()), "Valid");
    }
}
