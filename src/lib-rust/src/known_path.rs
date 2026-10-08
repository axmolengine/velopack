use anyhow::{bail, Result};
use std::path::PathBuf;
use crate::wide_strings::wide_to_os_string;
use windows::{
    core::GUID,
    Win32::UI::Shell::{
        FOLDERID_LocalAppData, SHGetKnownFolderPath
    },
};

#[cfg(windows)]
fn get_known_folder(rfid: *const GUID) -> Result<PathBuf> {
    unsafe {
        let flag = windows::Win32::UI::Shell::KNOWN_FOLDER_FLAG(0);
        let result = SHGetKnownFolderPath(rfid, flag, None)?;
        if result.is_null() {
            bail!("Failed to get known folder path (SHGetKnownFolderPath returned null)");
        }

        let str = wide_to_os_string(result);
        let path = PathBuf::from(str);
        Ok(path)
    }
}

#[cfg(windows)]
pub fn get_local_app_data() -> Result<PathBuf> {
    get_known_folder(&FOLDERID_LocalAppData)
}

/// The folder under LocalAppData that per-user applications are installed into.
/// There is no known folder for this path (FOLDERID_Programs is the Start Menu's Programs folder),
/// so it is derived by joining the literal onto LocalAppData.
#[cfg(windows)]
const PROGRAMS_FOLDER_NAME: &str = "Programs";

/// The shared `LocalAppData\Programs` directory, where per-user applications install themselves.
/// Each application owns a sub-directory named after its app id; the directory itself belongs to
/// no single application and is never deleted wholesale.
#[cfg(windows)]
pub fn get_local_app_programs_dir() -> Result<PathBuf> {
    Ok(get_local_app_data()?.join(PROGRAMS_FOLDER_NAME))
}

/// The directory a per-user install of the given app goes to by default.
/// Keep in sync with `Velopack.Windows.KnownPaths.GetAppRootDir` and
/// `MsiTemplateData.PerUserParentFolderName`, which describe the same location to the
/// C# runtime and to Windows Installer respectively.
#[cfg(windows)]
pub fn default_app_root_dir(app_id: &str) -> Result<PathBuf> {
    Ok(get_local_app_programs_dir()?.join(app_id))
}

/// The directory a per-user install went to before apps were moved under `Programs`.
/// Still needs to be recognised so existing installs are upgraded in place rather than
/// duplicated, and so their left-over cache can be cleaned up.
#[cfg(windows)]
pub fn legacy_app_root_dir(app_id: &str) -> Result<PathBuf> {
    Ok(get_local_app_data()?.join(app_id))
}

/// Every directory an app's per-user data might live in, most preferred first.
#[cfg(windows)]
pub fn app_root_candidates(app_id: &str) -> Vec<PathBuf> {
    [default_app_root_dir(app_id), legacy_app_root_dir(app_id)]
        .into_iter()
        .filter_map(Result::ok)
        .collect()
}



