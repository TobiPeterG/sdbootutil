use super::io::{get_command_output, log_info, print_error};
use super::*;
use libbtrfs::subvolume;
pub(crate) use std::env::consts::ARCH;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A structure representing an item subject to rollback actions.
///
/// This structure is used to encapsulate the path of a file that may need to be
/// restored to a previous state or cleaned up as part of a rollback process.
///
/// # Fields
///
/// * `original_path`: The `PathBuf` representing the path to the original file.
pub(crate) struct RollbackItem {
    original_path: PathBuf,
}

impl RollbackItem {
    /// Creates a new `RollbackItem` with the specified original file path.
    ///
    /// # Arguments
    ///
    /// * `original_path` - A `PathBuf` indicating the path to the original file that might be subject to rollback.
    ///
    /// # Returns
    ///
    /// Returns a new instance of `RollbackItem`.
    pub(crate) fn new(original_path: PathBuf) -> Self {
        RollbackItem { original_path }
    }

    /// Performs cleanup actions for the rollback item.
    ///
    /// If a backup file (with a `.bak` extension) exists, this function will attempt to restore the original file from the backup.
    /// If no backup is found but the original file exists, the original file will be removed.
    /// If neither the original file nor its backup exists, an informational message will be logged.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if cleanup actions are completed successfully, or an `IoError` if any file operations fail.
    pub(crate) fn cleanup(&self) -> std::io::Result<()> {
        let backup_path = self.original_path.with_extension("bak");
        if backup_path.exists() {
            fs::rename(&backup_path, &self.original_path).expect("Failed to restore from backup");
            let message = format!("restored {}", self.original_path.display());
            log_info(&message, 1);
        } else {
            if self.original_path.exists() {
                fs::remove_file(&self.original_path).expect("Failed to remove original file");
            } else {
                let message = format!(
                    "The following file doesn't exist and couldn't be removed: '{}'",
                    self.original_path.display()
                );
                log_info(&message, 1);
            }
        }
        Ok(())
    }
}

/// Creates a new temporary directory using the `tempfile` crate.
///
/// This function is typically used to create a temporary workspace for operations that require filesystem changes
/// which should not affect the permanent storage.
///
/// # Returns
///
/// Returns a tuple containing the `TempDir` object representing the temporary directory and its `PathBuf`.
/// The `TempDir` object ensures that the directory is deleted when it goes out of scope.
pub(crate) fn create_temp_dir() -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().expect("Failed to create a temporary directory");
    let temp_dir_path = temp_dir.path().to_path_buf();
    (temp_dir, temp_dir_path)
}

/// Cleans up a list of `RollbackItem`s.
///
/// Iterates over each `RollbackItem` in the provided slice and performs cleanup actions.
/// If any cleanup action fails, an error message will be logged.
///
/// # Arguments
///
/// * `rollback_items` - A slice of `RollbackItem`s to be cleaned up.
pub(crate) fn cleanup_rollback_items(rollback_items: &[RollbackItem]) {
    for item in rollback_items {
        if let Err(e) = item.cleanup() {
            let message = format!("Error cleaning up item: {}", e);
            print_error(&message);
        }
    }
}

/// Resets the state of a list of `RollbackItem`s by removing any associated backup files.
///
/// For each item in the list, if a backup file exists, it will be removed. After processing all items,
/// the list will be cleared, indicating that no rollback items remain to be processed.
///
/// # Arguments
///
/// * `rollback_items` - A mutable reference to a `Vec<RollbackItem>` representing the list of items to be reset.
pub(crate) fn reset_rollback_items(rollback_items: &mut Vec<RollbackItem>) {
    for item in rollback_items.iter() {
        let backup_path = item.original_path.with_extension("bak");
        if backup_path.exists() {
            if let Err(e) = fs::remove_file(&backup_path) {
                let message = format!(
                    "Failed to remove backup file {}: {}",
                    backup_path.display(),
                    e
                );
                print_error(&message);
            } else {
                let message = format!("Removed backup file {}", backup_path.display());
                log_info(&message, 1)
            }
        }
    }
    rollback_items.clear();
}

/// Checks if the filesystem type of `/etc` is `overlayfs`.
///
/// # Returns
///
/// `Ok(true)` if the filesystem type is `overlayfs`, `Ok(false)` otherwise, or an `Error` if the command execution fails.
pub(crate) fn is_transactional() -> Result<bool, Box<dyn Error>> {
    let filesystem_type = get_command_output("stat", &["-f", "-c", "%T", "/etc"])?;
    Ok(filesystem_type == "overlayfs")
}

/// Retrieves detailed information about the root Btrfs snapshot.
///
/// This function extracts and returns the prefix path, the snapshot ID, and the full snapshot path from the system's
/// root directory. It's designed to parse the snapshot path to identify these components, crucial for Btrfs snapshot management.
///
/// # Returns
///
/// A Result containing a tuple of:
/// - The prefix path as a String.
/// - The snapshot ID as a u64.
/// - The full snapshot path as a String.
///
/// # Errors
///
/// Returns an error if the snapshot path does not conform to the expected structure or if any parsing fails.
pub(crate) fn get_root_snapshot_info() -> Result<(String, u64, String), Box<dyn std::error::Error>>
{
    let full_path = subvolume::get_subvol_path("/")?;
    let parts: Vec<&str> = full_path.split("/.snapshots/").collect();
    let prefix = parts.get(0).ok_or("Prefix not found")?.to_string();
    let snapshot_part = parts.get(1).ok_or("Snapshot part not found")?;
    let snapshot_id_str = snapshot_part
        .split('/')
        .next()
        .ok_or("Snapshot ID not found")?;
    let snapshot_id = snapshot_id_str.parse::<u64>()?;

    Ok((prefix, snapshot_id, full_path))
}

/// Finds the path to the systemd-boot EFI file based on a given snapshot and firmware architecture,
/// with an optional prefix to override the default path for testing or other purposes.
///
/// # Arguments
///
/// * `snapshot` - A numeric identifier for the snapshot directory.
/// * `firmware_arch` - The architecture of the firmware, used to construct the EFI file name.
/// * `prefix_override` - An optional path to override the default "/.snapshots" prefix.
///
/// # Returns
///
/// Returns the `PathBuf` pointing to the systemd-boot EFI file.
pub(crate) fn find_sdboot(
    snapshot: u64,
    firmware_arch: &str,
    prefix_override: Option<&Path>,
) -> PathBuf {
    // Use the provided prefix if specified, otherwise default to "/.snapshots"
    let base_prefix = Path::new("/.snapshots");
    let prefix = prefix_override
        .unwrap_or(base_prefix)
        .join(snapshot.to_string())
        .join("snapshot");
    let mut sdboot_path = prefix.join(format!(
        "usr/lib/systemd-boot/systemd-boot{}.efi",
        firmware_arch
    ));

    if !sdboot_path.exists() {
        sdboot_path = prefix.join(format!(
            "usr/lib/systemd/boot/efi/systemd-boot{}.efi",
            firmware_arch
        ));
    }

    sdboot_path
}

/// Finds the path to the GRUB2 EFI file based on a given snapshot.
///
/// This function constructs a path within a specified snapshot directory to locate the GRUB2 EFI file.
/// It tries the primary expected location first and falls back to a secondary location if the EFI file is not found.
/// An optional override prefix can be provided for testing purposes.
///
/// # Arguments
///
/// * `snapshot` - A numeric identifier for the snapshot directory.
/// * `override_prefix` - An optional override prefix for the search, used primarily for testing.
///
/// # Returns
///
/// Returns the `PathBuf` pointing to the GRUB2 EFI file, whether it's in the primary or fallback location.
pub(crate) fn find_grub2(snapshot: u64, override_prefix: Option<&Path>) -> PathBuf {
    let base_prefix = Path::new("/.snapshots");
    let prefix = override_prefix
        .unwrap_or(base_prefix)
        .join(snapshot.to_string())
        .join("snapshot");
    let mut grub2_path = prefix.join(format!("usr/share/efi/{}/grub.efi", ARCH));

    if !grub2_path.exists() {
        grub2_path = prefix.join(format!("usr/share/grub2/{}-efi/grub.efi", ARCH));
    }
    grub2_path
}

/// Determines if the systemd-boot bootloader is installed for a given snapshot and firmware architecture.
///
/// This function checks for the presence of a systemd-boot EFI file and the absence of a GRUB2 EFI file
/// to determine if systemd-boot is the installed bootloader.
///
/// # Arguments
///
/// * `snapshot` - A numeric identifier for the snapshot directory.
/// * `firmware_arch` - The architecture of the firmware, used to check for the systemd-boot EFI file.
///
/// # Returns
///
/// Returns `true` if the systemd-boot EFI file exists and the GRUB2 EFI file does not, indicating systemd-boot is installed.
pub(crate) fn is_sdboot(
    snapshot: u64,
    firmware_arch: &str,
    override_prefix: Option<&Path>,
) -> bool {
    let sdboot = find_sdboot(snapshot, firmware_arch, override_prefix);
    let grub2 = find_grub2(snapshot, override_prefix);

    sdboot.exists() && !grub2.exists()
}

/// Determines if the GRUB2 bootloader is installed for a given snapshot.
///
/// This function checks for the presence of a GRUB2 EFI file to determine if GRUB2 is the installed bootloader.
///
/// # Arguments
///
/// * `snapshot` - A numeric identifier for the snapshot directory.
///
/// # Returns
///
/// Returns `true` if the GRUB2 EFI file exists, indicating GRUB2 is installed.
pub(crate) fn is_grub2(snapshot: u64, override_prefix: Option<&Path>) -> bool {
    find_grub2(snapshot, override_prefix).exists()
}

/// Determines the boot destination path based on the installed bootloader for a given snapshot.
///
/// This function checks whether the systemd-boot or GRUB2 bootloader is installed for
/// the specified snapshot and firmware architecture. It returns the appropriate boot destination path
/// based on the bootloader detected. The function supports overriding the default search prefix through an optional parameter.
///
/// # Arguments
///
/// * `snapshot` - A numeric identifier for the snapshot directory. This is used to locate the snapshot-specific bootloader files.
/// * `firmware_arch` - The architecture of the firmware, such as "x64" or "arm64".
/// This is used to refine the search for the bootloader files.
/// * `override_prefix` - An optional path override. If provided, this path will be used as the base directory
/// for searching bootloader files, instead of the default path.
///
/// # Returns
///
/// Returns `Ok("/EFI/systemd")` if systemd-boot is detected as the installed bootloader, `Ok("/EFI/opensuse")` if GRUB2 is detected,
/// or an `Err` with a message indicating that the bootloader is unsupported or could not be determined.
pub(crate) fn determine_boot_dst(
    snapshot: u64,
    firmware_arch: &str,
    override_prefix: Option<&Path>,
) -> Result<&'static str, &'static str> {
    if is_sdboot(snapshot, firmware_arch, override_prefix) {
        Ok("/EFI/systemd")
    } else if is_grub2(snapshot, override_prefix) {
        Ok("/EFI/opensuse")
    } else {
        Err("Unsupported bootloader or unable to determine bootloader")
    }
}

/// Finds the installed bootloader (systemd-boot or GRUB2) for a given snapshot and firmware architecture.
///
/// This function attempts to determine which bootloader is installed by checking for the presence of systemd-boot and GRUB2 EFI files.
/// It favors systemd-boot unless only GRUB2 is found.
///
/// # Arguments
///
/// * `snapshot` - A numeric identifier for the snapshot directory.
/// * `firmware_arch` - The architecture of the firmware, used in the search for the systemd-boot EFI file.
///
/// # Returns
///
/// Returns a `Result` containing a `PathBuf` to the detected bootloader EFI file on success,
/// or an error string if no bootloader is detected.
pub(crate) fn find_bootloader(
    snapshot: u64,
    firmware_arch: &str,
    override_prefix: Option<&Path>,
) -> Result<PathBuf, &'static str> {
    if is_sdboot(snapshot, firmware_arch, override_prefix) {
        Ok(find_sdboot(snapshot, firmware_arch, override_prefix))
    } else if is_grub2(snapshot, override_prefix) {
        Ok(find_grub2(snapshot, override_prefix))
    } else {
        Err("Bootloader not detected")
    }
}

pub(crate) fn find_version(
    content: &[u8],
    start_pattern: &[u8],
    end_pattern: &[u8],
) -> Option<String> {
    if let Some(start_pos) = content
        .windows(start_pattern.len())
        .position(|window| window == start_pattern)
    {
        let version_start_pos = start_pos + start_pattern.len();
        if let Some(end_pos) = content[version_start_pos..]
            .windows(end_pattern.len())
            .position(|window| window == end_pattern)
        {
            let version_bytes = &content[version_start_pos..version_start_pos + end_pos];
            return std::str::from_utf8(version_bytes).ok().map(str::to_string);
        }
    }
    None
}

pub(crate) fn bootloader_version(
    snapshot: u64,
    firmware_arch: &str,
    shimdir: &str,
    boot_root: &str,
    boot_dst: &str,
    filename: Option<PathBuf>,
    override_prefix: Option<&Path>,
) -> Result<String, String> {
    let prefix = override_prefix.unwrap_or(Path::new(""));
    let fn_path = match filename {
        Some(f) => f,
        None => {
            if PathBuf::from(format!("{}{}/shim.efi", prefix.display(), shimdir)).exists() {
                PathBuf::from(format!(
                    "{}{}{}/grub.efi",
                    prefix.display(),
                    boot_root,
                    boot_dst
                ))
            } else {
                let bootloader = find_bootloader(snapshot, firmware_arch, override_prefix)?;
                PathBuf::from(format!(
                    "{}{}{}/{}",
                    prefix.display(),
                    boot_root,
                    boot_dst,
                    bootloader.file_name().unwrap().to_str().unwrap()
                ))
            }
        }
    };
    if !fn_path.exists() {
        let err = format!("File does not exist: {}", fn_path.display());
        return Err(err);
    }

    let content = fs::read(&fn_path).map_err(|_| "Failed to read file")?;

    let patterns = [
        (&b"LoaderInfo: systemd-boot "[..], &b" ####"[..]),
        (&b"GNU GRUB  version %s\x00"[..], &b"\x00"[..]),
    ];
    for (start, end) in &patterns {
        if let Some(version) = find_version(&content, start, end) {
            return Ok(version);
        }
    }
    Err("Version not found".to_string())
}

pub(crate) fn is_installed(
    snapshot: u64,
    firmware_arch: &str,
    shimdir: &str,
    boot_root: &str,
    boot_dst: &str,
    filename: Option<PathBuf>,
    override_prefix: Option<&Path>,
) -> bool {
    let prefix = override_prefix.unwrap_or(Path::new(""));
    let bootloader_version_successful = bootloader_version(
        snapshot,
        firmware_arch,
        shimdir,
        boot_root,
        boot_dst,
        filename,
        override_prefix,
    )
    .is_ok();
    let flag_path = format!(
        "{}{}{}/installed_by_sdbootutil",
        prefix.display(),
        boot_root,
        boot_dst
    );
    let installed_flag_path = Path::new(&flag_path);
    let installed_flag_exists = installed_flag_path.exists();

    bootloader_version_successful && installed_flag_exists
}
