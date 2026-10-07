//! OS integration: file manager, default application, quick locations.

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "macos")]
pub const FILE_MANAGER: &str = "Finder";
#[cfg(windows)]
pub const FILE_MANAGER: &str = "Explorer";
#[cfg(not(any(target_os = "macos", windows)))]
pub const FILE_MANAGER: &str = "File Manager";

/// Shows `path` selected in the system file manager.
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    spawn(Command::new("open").arg("-R").arg(path));

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        spawn(Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())));
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        // Ask the file manager over D-Bus to select the item, fall back to opening the folder.
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let uri = file_uri(&path);
            let selected = Command::new("dbus-send")
                .args([
                    "--session",
                    "--print-reply",
                    "--dest=org.freedesktop.FileManager1",
                    "--type=method_call",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems",
                ])
                .arg(format!("array:string:{uri}"))
                .arg("string:")
                .output()
                .is_ok_and(|out| out.status.success());
            if !selected {
                let dir = if path.is_dir() { path.as_path() } else { path.parent().unwrap_or(&path) };
                let _ = Command::new("xdg-open").arg(dir).status();
            }
        });
    }
}

/// Opens `path` with its default application.
pub fn open(path: &Path) {
    #[cfg(target_os = "macos")]
    spawn(Command::new("open").arg(path));

    #[cfg(windows)]
    spawn(Command::new("explorer").arg(path));

    #[cfg(not(any(target_os = "macos", windows)))]
    spawn(Command::new("xdg-open").arg(path));
}

/// Moves `path` to the system trash / recycle bin.
pub fn move_to_trash(path: &Path) -> Result<(), trash::Error> {
    #[cfg(target_os = "macos")]
    let ctx = {
        // Asking Finder (the default) needs an Automation permission prompt; the file
        // manager API doesn't.
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx
    };
    #[cfg(not(target_os = "macos"))]
    let ctx = trash::TrashContext::default();
    ctx.delete(path)
}

fn spawn(cmd: &mut Command) {
    // Reap the child in the background so it doesn't linger as a zombie.
    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || child.wait());
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// Top-level locations worth offering on the start screen.
pub fn volumes() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
        const DRIVE_UNKNOWN: u32 = 0;
        const DRIVE_NO_ROOT_DIR: u32 = 1;

        // Ask for the drive letters instead of probing each path: touching a disconnected
        // network drive or an empty card reader can block for seconds.
        // SAFETY: plain Win32 calls; the path passed is NUL-terminated.
        let mask = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char)))
            .filter(|p| {
                let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
                let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
                kind != DRIVE_UNKNOWN && kind != DRIVE_NO_ROOT_DIR
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        fn subdirs(dir: &Path) -> impl Iterator<Item = PathBuf> {
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                // Skip symlinks such as /Volumes/Macintosh HD -> /.
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
        }

        let mut out = vec![PathBuf::from("/")];
        #[cfg(target_os = "macos")]
        out.extend(subdirs(Path::new("/Volumes")));
        #[cfg(not(target_os = "macos"))]
        {
            // Removable drives are mounted at /media/$USER/<disk> or /run/media/$USER/<disk>.
            let user = std::env::var_os("USER").unwrap_or_default();
            for base in ["/media", "/run/media", "/mnt"] {
                for dir in subdirs(Path::new(base)) {
                    if dir.file_name() == Some(user.as_os_str()) {
                        out.extend(subdirs(&dir));
                    } else {
                        out.push(dir);
                    }
                }
            }
        }
        out
    }
}

/// Free and total bytes of the volume holding `path`.
#[cfg(unix)]
#[allow(clippy::useless_conversion)] // statvfs field types differ between platforms
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid C string and `stat` a properly sized out-parameter.
    let stat = unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(path.as_ptr(), &mut stat) != 0 {
            return None;
        }
        stat
    };
    let block = u64::from(stat.f_frsize);
    Some((u64::from(stat.f_bavail) * block, u64::from(stat.f_blocks) * block))
}

/// Free and total bytes of the volume holding `path`.
#[cfg(windows)]
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated; the out-pointers are valid for the call.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    (ok != 0).then_some((free, total))
}

#[cfg(test)]
mod tests {
    #[test]
    fn disk_space_of_temp_dir() {
        let (free, total) = super::disk_space(&std::env::temp_dir()).expect("disk space");
        assert!(total > 0);
        assert!(free <= total);
    }
}
