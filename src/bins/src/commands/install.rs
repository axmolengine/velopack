use crate::setup_errors::SetupError;
use crate::{dialogs, shared, windows};
use velopack::constants;
use velopack::locator::*;
use velopack::{bundle::BundleZip, wide_strings::string_to_wide, windows_channel_tag};

use ::windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use anyhow::Result;
use pretty_bytes_rust::{pretty_bytes, PrettyBytesOptions};
use std::{
    ffi::OsString,
    fs::{self},
    path::{Path, PathBuf},
};

/// Installs the given package. If `channel_override` is set, the installed manifest's `<channel>`
/// is rewritten to it before any hooks run or the app is launched.
pub fn install(pkg: &mut BundleZip, install_to: Option<&PathBuf>, start_args: Option<Vec<OsString>>, channel_override: Option<&str>) -> Result<()> {
    // find and parse nuspec
    info!("Reading package manifest...");
    let app = pkg.read_manifest()?;

    info!("Package manifest loaded successfully.");
    info!("    Package ID: {}", app.id);
    info!("    Package Version: {}", app.version);
    info!("    Package Title: {}", app.title);
    info!("    Package Authors: {}", app.authors);
    info!("    Package Description: {}", app.description);
    info!("    Package Machine Architecture: {}", app.machine_architecture);
    info!("    Package Runtime Dependencies: {}", app.runtime_dependencies);

    if !windows::prerequisite::prompt_and_install_all_missing(&app.title, &app.version.to_string(), &app.runtime_dependencies, None)? {
        info!("Cancelling setup. Pre-requisites not installed.");
        return Ok(());
    }

    info!("Determining install directory...");
    let (root_path, stale_root) = match install_to {
        Some(path) => (path.clone(), None),
        None => choose_default_root(&app.id)?,
    };

    // path needs to exist for future operations (disk space etc)
    if !root_path.exists() {
        shared::retry_io(|| fs::create_dir_all(&root_path))?;
    }

    info!("Installation Directory: {:?}", root_path);

    // do we have enough disk space?
    let (compressed_size, extracted_size) = pkg.calculate_size();
    let required_space = compressed_size + extracted_size + (50 * 1000 * 1000); // archive + velopack overhead

    let mut free_space: u64 = 0;
    let root_pcwstr = string_to_wide(&root_path);
    if let Ok(()) = unsafe { GetDiskFreeSpaceExW(root_pcwstr.as_pcwstr(), None, None, Some(&mut free_space)) } {
        if free_space < required_space {
            return Err(SetupError::InsufficientDiskSpace {
                app_title: app.title.clone(),
                required_space: format_disk_space(required_space),
                available_space: format_disk_space(free_space),
            }
            .into());
        }
    }

    info!(
        "There is {} free space available at destination, this package requires {}.",
        format_disk_space(free_space),
        format_disk_space(required_space)
    );

    // does this app support this OS / architecture?
    if !app.os_min_version.is_empty() && !windows::is_os_version_or_greater(&app.os_min_version)? {
        return Err(SetupError::OsVersionRequired {
            app_title: app.title.clone(),
            os_version: app.os_min_version.clone(),
        }
        .into());
    }

    if !app.machine_architecture.is_empty() && !windows::is_cpu_architecture_supported(&app.machine_architecture)? {
        return Err(SetupError::CpuArchUnsupported {
            app_title: app.title.clone(),
            machine_arch: app.machine_architecture.clone(),
        }
        .into());
    }

    let mut root_path_renamed: Option<PathBuf> = None;
    // does the target directory exist and have files? (eg. already installed)
    if !shared::is_dir_empty(&root_path) {
        // the target directory is not empty, and not dead
        let installed_version = auto_locate_app_manifest(LocationContext::FromSpecifiedRootDir(root_path.clone(), None))
            .ok()
            .map(|loc| loc.get_manifest_version().clone());
        if !dialogs::show_overwrite_repair_dialog(&app.title, &app.version, &root_path, installed_version.as_ref()) {
            // user cancelled overwrite prompt
            error!("Directory already exists, and user cancelled overwrite.");
            return Ok(());
        }
        info!("User chose to overwrite existing installation.");

        shared::force_stop_package(&root_path).map_err(|z| SetupError::StopApplicationFailed {
            app_title: app.title.clone(),
            error: z.to_string(),
        })?;

        let renamed = root_path.with_extension(shared::random_string(16));
        info!("Renaming existing directory to '{:?}' to allow rollback...", renamed);

        shared::retry_io(|| fs::rename(&root_path, &renamed)).map_err(|_| SetupError::RemoveExistingDirFailed {
            app_title: app.title.clone(),
        })?;

        root_path_renamed = Some(renamed);
    }

    info!("Preparing and cleaning installation directory...");
    remove_dir_all::ensure_empty_dir(&root_path)?;

    info!("Acquiring lock...");
    let paths = create_config_from_root_dir(&root_path);
    let locator = VelopackLocator::new_with_manifest(paths, app);
    let _mutex = locator.try_get_exclusive_lock()?;

    let tx = if dialogs::get_silent() {
        info!("Will not show splash because silent mode is on.");
        let (tx, _) = std::sync::mpsc::channel::<i16>();
        tx
    } else {
        info!("Reading splash image...");
        let manifest = locator.get_manifest();
        let splash_bytes = pkg.get_splash_bytes();
        windows::splash::show_splash_dialog(
            manifest.title,
            manifest.version.to_string(),
            splash_bytes,
            windows::splash::SplashOptions {
                splash_progress_color: Some(manifest.splash_progress_color),
            },
        )
    };

    let install_result = install_impl(pkg, &locator, &tx, start_args, channel_override);
    let _ = tx.send(windows::splash::MSG_CLOSE);

    if install_result.is_ok() {
        info!("Installation completed successfully!");
        if let Some(renamed) = root_path_renamed {
            info!("Removing rollback directory...");
            let _ = shared::retry_io(|| remove_dir_all::remove_dir_all(&renamed));
        }
        if let Some(stale_root) = &stale_root {
            // Only once the new shortcuts exist, so the app is never left without one. The
            // directory itself is the user's and is left alone.
            info!("Removing shortcuts of the superseded installation at {:?}", stale_root);
            windows::remove_all_shortcuts_for_root_dir(stale_root);
        }
    } else {
        error!("Installation failed!");
        if let Some(renamed) = root_path_renamed {
            info!("Rolling back installation...");
            let _ = shared::force_stop_package(&root_path);
            let _ = shared::retry_io(|| remove_dir_all::remove_dir_all(&root_path));
            let _ = shared::retry_io(|| fs::rename(&renamed, &root_path));
        }
        install_result?;
    }

    Ok(())
}

/// Picks where a per-user install goes, and returns the directory of a previous installation
/// whose shortcuts must be removed alongside it.
fn choose_default_root(app_id: &str) -> Result<(PathBuf, Option<PathBuf>)> {
    let default_root = velopack::known_path::default_app_root_dir(app_id)?;
    let legacy_root = velopack::known_path::legacy_app_root_dir(app_id)?;
    Ok(select_root(&default_root, &legacy_root))
}

/// Apps used to be installed directly in LocalAppData rather than in a `Programs` sub-directory,
/// so an existing install there is upgraded in place instead of being left behind as a second copy
/// that the old one's shortcuts would still launch. Split out from `choose_default_root` so the
/// decision can be tested without writing into a real user's profile.
fn select_root(default_root: &Path, legacy_root: &Path) -> (PathBuf, Option<PathBuf>) {
    if !is_existing_app_install(legacy_root) {
        return (default_root.to_path_buf(), None);
    }

    if shared::is_dir_empty(default_root) {
        info!("Upgrading installation found at the previous location: {:?}", legacy_root);
        return (legacy_root.to_path_buf(), None);
    }

    (default_root.to_path_buf(), Some(legacy_root.to_path_buf()))
}

/// True if the app is (or was) installed here, as opposed to only keeping downloaded packages here.
/// An install with a read-only root - a Program Files MSI, for instance - leaves just an
/// `Update.exe` and a `packages` directory in the user's LocalAppData, with no `current` directory.
/// That must not be adopted as an install root, or it would be renamed away and the other
/// install's downloads destroyed. The `app-*` case is a legacy Squirrel/Clowd layout, which is
/// migrated in place today and has no manifest to check for.
fn is_existing_app_install(dir: &Path) -> bool {
    velopack::locator::is_velopack_root(dir) || (dir.join("Update.exe").is_file() && shared::has_app_prefixed_folder(dir))
}

fn format_disk_space(bytes: u64) -> String {
    pretty_bytes(
        bytes,
        Some(PrettyBytesOptions {
            use_1024_instead_of_1000: Some(false),
            number_of_decimal: Some(2),
            remove_zero_decimal: Some(false),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_disk_space_uses_byte_units() {
        assert_eq!(super::format_disk_space(832_980_000), "832.98 MB");
    }

    fn write_installed_app(dir: &Path) {
        std::fs::create_dir_all(dir.join("current")).unwrap();
        std::fs::write(dir.join("Update.exe"), b"update").unwrap();
        std::fs::write(dir.join("current").join("sq.version"), b"nuspec").unwrap();
    }

    /// A Squirrel/Clowd install predates `current` and `sq.version`.
    fn write_legacy_app(dir: &Path) {
        std::fs::create_dir_all(dir.join("app-1.0.0")).unwrap();
        std::fs::write(dir.join("Update.exe"), b"update").unwrap();
    }

    /// What an install with a read-only root leaves behind: no `current`, so not an install.
    fn write_package_cache(dir: &Path) {
        std::fs::create_dir_all(dir.join("packages")).unwrap();
        std::fs::write(dir.join("Update.exe"), b"update").unwrap();
    }

    #[test]
    fn a_clean_machine_installs_under_programs() {
        let tmp = tempfile::tempdir().unwrap();
        let default_root = tmp.path().join("Programs").join("CoolApp");
        let legacy_root = tmp.path().join("CoolApp");

        let (root, stale) = select_root(&default_root, &legacy_root);

        assert_eq!(root, default_root);
        assert_eq!(stale, None);
    }

    #[test]
    fn an_existing_install_is_upgraded_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let default_root = tmp.path().join("Programs").join("CoolApp");
        let legacy_root = tmp.path().join("CoolApp");
        write_installed_app(&legacy_root);

        let (root, stale) = select_root(&default_root, &legacy_root);

        assert_eq!(root, legacy_root);
        assert_eq!(stale, None);
    }

    #[test]
    fn a_legacy_squirrel_install_is_upgraded_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let default_root = tmp.path().join("Programs").join("CoolApp");
        let legacy_root = tmp.path().join("CoolApp");
        write_legacy_app(&legacy_root);

        let (root, _) = select_root(&default_root, &legacy_root);

        assert_eq!(root, legacy_root);
    }

    #[test]
    fn a_package_cache_is_not_adopted_as_an_install() {
        let tmp = tempfile::tempdir().unwrap();
        let default_root = tmp.path().join("Programs").join("CoolApp");
        let legacy_root = tmp.path().join("CoolApp");
        write_package_cache(&legacy_root);

        let (root, stale) = select_root(&default_root, &legacy_root);

        assert_eq!(root, default_root);
        assert_eq!(stale, None);
    }

    #[test]
    fn an_installed_programs_root_wins_and_the_old_shortcuts_go() {
        let tmp = tempfile::tempdir().unwrap();
        let default_root = tmp.path().join("Programs").join("CoolApp");
        let legacy_root = tmp.path().join("CoolApp");
        write_installed_app(&legacy_root);
        std::fs::create_dir_all(default_root.join("current")).unwrap();

        let (root, stale) = select_root(&default_root, &legacy_root);

        assert_eq!(root, default_root);
        assert_eq!(stale, Some(legacy_root));
    }
}

fn install_impl(
    pkg: &mut BundleZip,
    locator: &VelopackLocator,
    tx: &std::sync::mpsc::Sender<i16>,
    start_args: Option<Vec<OsString>>,
    channel_override: Option<&str>,
) -> Result<()> {
    info!("Starting installation!");

    // all application paths
    let updater_path = locator.get_update_path();
    let packages_path = locator.get_packages_dir();
    let current_path = locator.get_current_bin_dir();
    let nupkg_path = locator.get_ideal_local_nupkg_path(None, None);
    let main_exe_path = locator.get_main_exe_path();

    info!("Extracting Update.exe...");
    let _ = pkg
        .extract_zip_predicate_to_path(|name| name.ends_with("Squirrel.exe"), updater_path)
        .map_err(|_| SetupError::UpdateExeMissing {
            app_title: locator.get_manifest_title(),
        })?;

    let _ = pkg.extract_stubs_to_dir(locator.get_root_dir());
    let _ = tx.send(5);

    info!("Copying nupkg to packages directory...");
    shared::retry_io(|| fs::create_dir_all(&packages_path))?;
    pkg.copy_bundle_to_file(&nupkg_path)?;
    let _ = tx.send(10);

    pkg.extract_lib_contents_to_path(&current_path, |p| {
        let _ = tx.send(((p as f32) / 100.0 * 80.0 + 10.0) as i16);
    })?;

    // must happen before the install hook runs / the app is launched, so they see the new channel
    if let Some(channel) = channel_override {
        info!("Applying channel override '{}' to installed manifest...", channel);
        if let Err(e) = windows_channel_tag::patch_installed_channel(&locator.get_root_dir(), channel) {
            warn!("Failed to apply channel override (non-fatal): {}", e);
        }
    }

    if !main_exe_path.exists() {
        return Err(SetupError::MainExeMissing {
            app_title: locator.get_manifest_title(),
        }
        .into());
    }

    if locator.get_manifest_shortcut_locations() != ShortcutLocationFlags::NONE {
        info!("Creating shortcuts...");
        windows::create_or_update_manifest_lnks(locator, None);
    }

    info!("Starting process install hook");
    if !windows::run_hook(locator, constants::HOOK_CLI_INSTALL, 30) {
        dialogs::show_install_hook_warning(&locator.get_manifest_title());
    }

    let _ = tx.send(100);
    windows::registry::write_uninstall_entry(locator)?;

    if !dialogs::get_silent() {
        info!("Starting app...");
        shared::start_package(locator, start_args, Some(constants::HOOK_ENV_FIRSTRUN))?;
    }

    Ok(())
}
