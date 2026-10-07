//! Parallel directory scanner running on a background thread.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::tree::{Kind, ScanNode, Tree};

/// Live counters shared between the scanner and the UI.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub current: Mutex<PathBuf>,
    cancel: AtomicBool,
}

impl Progress {
    fn cancelled(&self) -> bool {
        self.cancel.load(Relaxed)
    }
}

pub enum Outcome {
    Done(Tree),
    Cancelled,
    /// The root itself could not be read.
    Failed(String),
}

pub struct Finished {
    pub outcome: Outcome,
    pub errors: u64,
    pub elapsed: Duration,
}

pub struct Scan {
    pub root: PathBuf,
    pub progress: Arc<Progress>,
    pub started: Instant,
    rx: Receiver<Finished>,
}

impl Scan {
    /// Starts scanning `root` in the background; `on_done` is called from the
    /// scanner thread once the result is ready (used to wake up the UI).
    pub fn start(root: PathBuf, on_done: impl FnOnce() + Send + 'static) -> Self {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let started = Instant::now();
        {
            let root = root.clone();
            let progress = Arc::clone(&progress);
            std::thread::Builder::new()
                .name("scanner".into())
                .spawn(move || {
                    let outcome = match scan(&root, &progress) {
                        _ if progress.cancelled() => Outcome::Cancelled,
                        Ok(tree) => Outcome::Done(tree),
                        Err(e) => Outcome::Failed(format!("Cannot read {}: {e}", root.display())),
                    };
                    let _ = tx.send(Finished {
                        outcome,
                        errors: progress.errors.load(Relaxed),
                        elapsed: started.elapsed(),
                    });
                    on_done();
                })
                .expect("failed to spawn scanner thread");
        }
        Self { root, progress, started, rx }
    }

    pub fn cancel(&self) {
        self.progress.cancel.store(true, Relaxed);
    }

    /// Returns the result once the scan has finished.
    pub fn poll(&self) -> Option<Finished> {
        match self.rx.try_recv() {
            Ok(done) => Some(done),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Finished {
                outcome: Outcome::Failed("The scanner stopped unexpectedly".into()),
                errors: self.progress.errors.load(Relaxed),
                elapsed: self.started.elapsed(),
            }),
        }
    }
}

/// Scans synchronously. Exposed for tests; the app goes through [`Scan::start`].
pub fn scan(root: &Path, progress: &Progress) -> io::Result<Tree> {
    let ctx = Ctx { progress, filter: Filter::new(root), hardlinks: Mutex::new(HashSet::new()) };
    let name: Box<OsStr> = root.as_os_str().into();

    let meta = fs::metadata(root)?;
    let node = if meta.is_dir() {
        fs::read_dir(root)?; // surface "permission denied" on the root itself
        {
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
            // Scanning is mostly waiting on the file system, so oversubscribe the CPUs.
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads((threads * 2).clamp(4, 32))
                .stack_size(8 * 1024 * 1024)
                .thread_name(|i| format!("scan-{i}"))
                .build();
            match pool {
                Ok(pool) => pool.install(|| scan_dir(&ctx, root, name, disk_size(&meta))),
                Err(_) => scan_dir(&ctx, root, name, disk_size(&meta)),
            }
        }
    } else {
        progress.files.fetch_add(1, Relaxed);
        ScanNode { name, size: disk_size(&meta), files: 1, kind: Kind::File, children: vec![] }
    };
    Ok(Tree::from_scan(root.to_path_buf(), node))
}

struct Ctx<'a> {
    progress: &'a Progress,
    filter: Filter,
    /// (device, inode) of multiply-linked files that were already counted.
    hardlinks: Mutex<HashSet<(u64, u64)>>,
}

fn scan_dir(ctx: &Ctx, path: &Path, name: Box<OsStr>, own_size: u64) -> ScanNode {
    let mut node = ScanNode { name, size: own_size, files: 0, kind: Kind::Dir, children: Vec::new() };
    let progress = ctx.progress;
    if progress.cancelled() {
        return node;
    }

    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => {
            progress.errors.fetch_add(1, Relaxed);
            return node;
        }
    };

    let mut subdirs = Vec::new();
    let (mut files, mut bytes, mut errors) = (0u64, own_size, 0u64);
    for entry in entries {
        let Ok(entry) = entry else {
            errors += 1;
            continue;
        };
        // DirEntry::metadata does not follow symlinks, so links are never traversed.
        let Ok(meta) = entry.metadata() else {
            errors += 1;
            continue;
        };
        if meta.is_dir() {
            let child_path = entry.path();
            if ctx.filter.allows(&child_path, &meta) {
                subdirs.push((child_path, entry.file_name().into_boxed_os_str(), disk_size(&meta)));
            }
        } else if ctx.first_link(&meta) {
            let size = disk_size(&meta);
            files += 1;
            bytes += size;
            node.children.push(ScanNode {
                name: entry.file_name().into_boxed_os_str(),
                size,
                files: 1,
                kind: Kind::File,
                children: Vec::new(),
            });
        }
    }

    progress.files.fetch_add(files, Relaxed);
    progress.bytes.fetch_add(bytes, Relaxed);
    if errors > 0 {
        progress.errors.fetch_add(errors, Relaxed);
    }
    if progress.dirs.fetch_add(1, Relaxed).is_multiple_of(64)
        && let Ok(mut current) = progress.current.try_lock()
    {
        current.clear();
        current.push(path);
    }

    let subdirs: Vec<ScanNode> = subdirs
        .into_par_iter()
        .map(|(child_path, name, size)| scan_dir(ctx, &child_path, name, size))
        .collect();
    node.children.extend(subdirs);
    node.finish();
    node
}

impl Ctx<'_> {
    /// False for a hard link whose inode was already counted elsewhere.
    #[cfg(unix)]
    fn first_link(&self, meta: &Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() <= 1 {
            return true;
        }
        self.hardlinks.lock().map_or(true, |mut seen| seen.insert((meta.dev(), meta.ino())))
    }

    #[cfg(not(unix))]
    fn first_link(&self, _meta: &Metadata) -> bool {
        let _ = &self.hardlinks;
        true
    }
}

#[cfg(unix)]
fn disk_size(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks() * 512
}

#[cfg(not(unix))]
fn disk_size(meta: &Metadata) -> u64 {
    meta.len()
}

/// Keeps the scan on the file system(s) of the root, like `du -x`.
struct Filter {
    #[cfg(unix)]
    devices: Vec<u64>,
    skip: Vec<PathBuf>,
}

impl Filter {
    #[cfg(unix)]
    fn new(root: &Path) -> Self {
        use std::os::unix::fs::MetadataExt;
        let mut devices = Vec::new();
        let mut skip = Vec::new();
        if let Ok(meta) = fs::metadata(root) {
            devices.push(meta.dev());
        }
        if cfg!(target_os = "macos") {
            // Since Catalina "/" is a read-only system volume with user data on a
            // separate volume, firmlinked into /Users, /Applications, /private, ...
            // Allow both volumes, but skip the raw mounts under /System/Volumes to
            // avoid counting the data twice.
            let volumes = Path::new("/System/Volumes");
            if !root.starts_with(volumes) {
                if let Ok(data) = fs::metadata(volumes.join("Data")) {
                    devices.push(data.dev());
                }
                skip.push(volumes.to_path_buf());
            }
        }
        #[cfg(target_os = "linux")]
        if let Ok(info) = fs::read_to_string("/proc/self/mountinfo") {
            // btrfs subvolumes (e.g. /home on Fedora) have their own device id but live
            // on the same disk, so treat them as part of the root's file system.
            for mount in btrfs_subvolume_mounts(&info, root) {
                if let Ok(meta) = fs::metadata(&mount) {
                    devices.push(meta.dev());
                }
            }
        }
        Self { devices, skip }
    }

    #[cfg(not(unix))]
    fn new(_root: &Path) -> Self {
        Self { skip: Vec::new() }
    }

    fn allows(&self, path: &Path, meta: &Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !self.devices.is_empty() && !self.devices.contains(&meta.dev()) {
                return false;
            }
        }
        #[cfg(not(unix))]
        let _ = meta;
        !self.skip.iter().any(|s| s == path)
    }
}

/// Mount points below `root` of other btrfs subvolumes from the same device as `root`,
/// read from `/proc/self/mountinfo`. Snapshots are left out so they aren't counted twice.
#[cfg(any(target_os = "linux", test))]
fn btrfs_subvolume_mounts(mountinfo: &str, root: &Path) -> Vec<PathBuf> {
    // Mount points escape spaces etc. as octal: "\040".
    fn unescape(field: &str) -> PathBuf {
        let bytes = field.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let octal = bytes.get(i + 1..i + 4).and_then(|d| std::str::from_utf8(d).ok());
            match octal.and_then(|d| u8::from_str_radix(d, 8).ok()) {
                Some(b) if bytes[i] == b'\\' => {
                    out.push(b);
                    i += 4;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        }
        PathBuf::from(String::from_utf8_lossy(&out).into_owned())
    }

    // "36 35 98:0 /subvol /mnt/point rw,noatime shared:1 - btrfs /dev/sda2 rw"
    let mounts: Vec<(PathBuf, &str, &str)> = mountinfo
        .lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let mount_point = unescape(left.split(' ').nth(4)?);
            let mut right = right.split(' ');
            Some((mount_point, right.next()?, right.next()?))
        })
        .collect();
    let Some((root_mount, fs_type, source)) =
        mounts.iter().filter(|(mp, ..)| root.starts_with(mp)).max_by_key(|(mp, ..)| mp.as_os_str().len())
    else {
        return Vec::new();
    };
    if *fs_type != "btrfs" {
        return Vec::new();
    }
    mounts
        .iter()
        .filter(|(mp, t, src)| {
            t == fs_type
                && src == source
                && mp != root_mount
                && mp.starts_with(root)
                && !mp.components().any(|c| c.as_os_str() == ".snapshots")
        })
        .map(|(mp, ..)| mp.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::NodeId;

    #[test]
    fn finds_btrfs_subvolumes_of_the_same_device() {
        let info = "\
22 1 0:21 /root / rw,relatime shared:1 - btrfs /dev/nvme0n1p3 rw,subvol=/root
23 22 0:5 / /dev rw - devtmpfs devtmpfs rw
45 22 0:21 /home /home rw,relatime shared:2 - btrfs /dev/nvme0n1p3 rw,subvol=/home
46 22 0:21 /snap /.snapshots rw - btrfs /dev/nvme0n1p3 rw,subvol=/.snapshots
47 22 0:40 / /mnt/usb rw - btrfs /dev/sdb1 rw
48 22 0:21 /data /srv/my\\040data rw - btrfs /dev/nvme0n1p3 rw
";
        let mounts = btrfs_subvolume_mounts(info, Path::new("/"));
        assert_eq!(mounts, [PathBuf::from("/home"), PathBuf::from("/srv/my data")]);
        assert!(btrfs_subvolume_mounts(info, Path::new("/home/me")).is_empty());
        assert!(btrfs_subvolume_mounts(info, Path::new("/dev")).is_empty());
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("disk-usage-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn child_named(tree: &Tree, parent: NodeId, name: &str) -> NodeId {
        tree.children(parent).find(|&c| tree.node(c).name_lossy() == name).unwrap()
    }

    #[test]
    fn scans_a_directory() {
        let root = temp_dir("scan");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("big.bin"), vec![1u8; 64 * 1024]).unwrap();
        fs::write(root.join("a/one.txt"), b"hello").unwrap();
        fs::write(root.join("a/b/two.txt"), vec![2u8; 8 * 1024]).unwrap();
        fs::create_dir(root.join("empty")).unwrap();

        let progress = Progress::default();
        let tree = scan(&root, &progress).unwrap();

        let r = Tree::ROOT;
        assert_eq!(tree.node(r).files, 3);
        assert_eq!(progress.files.load(Relaxed), 3);
        assert_eq!(progress.errors.load(Relaxed), 0);
        assert_eq!(tree.path(r), root);

        let first = tree.children(r).next().unwrap();
        assert_eq!(tree.node(first).name_lossy(), "big.bin");
        assert!(tree.node(first).size >= 64 * 1024);

        let a = child_named(&tree, r, "a");
        let b = child_named(&tree, a, "b");
        let two = child_named(&tree, b, "two.txt");
        assert_eq!(tree.path(two), root.join("a/b/two.txt"));
        assert_eq!(tree.node(a).files, 2);
        let children_sum: u64 = tree.children(r).map(|c| tree.node(c).size).sum();
        assert!(tree.node(r).size >= children_sum);

        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn counts_hardlinks_once_and_skips_symlinks() {
        let root = temp_dir("links");
        fs::write(root.join("data.bin"), vec![1u8; 32 * 1024]).unwrap();
        fs::hard_link(root.join("data.bin"), root.join("link.bin")).unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/x.bin"), vec![1u8; 32 * 1024]).unwrap();
        std::os::unix::fs::symlink(root.join("sub"), root.join("sub-link")).unwrap();

        let tree = scan(&root, &Progress::default()).unwrap();
        // data.bin/link.bin counted once, sub/x.bin once, the symlink is a tiny file.
        let sized: Vec<_> = tree.children(Tree::ROOT).filter(|&c| tree.node(c).size >= 32 * 1024).collect();
        assert_eq!(sized.len(), 2);
        let sub_link = child_named(&tree, Tree::ROOT, "sub-link");
        assert!(!tree.node(sub_link).is_dir());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_root_is_an_error() {
        assert!(scan(Path::new("/definitely/not/here/42"), &Progress::default()).is_err());
        assert!(scan(Path::new(""), &Progress::default()).is_err());
    }
}

/// `SCAN_BENCH=/some/path cargo test --release bench_scan -- --ignored --nocapture`
#[cfg(test)]
#[test]
#[ignore]
fn bench_scan() {
    let path = PathBuf::from(std::env::var_os("SCAN_BENCH").expect("set SCAN_BENCH to a directory"));
    let progress = Progress::default();
    let start = Instant::now();
    let tree = scan(&path, &progress).unwrap();
    println!(
        "{}: {} nodes, {} KiB, {} errors in {:.2?}",
        path.display(),
        tree.len(),
        tree.node(Tree::ROOT).size / 1024,
        progress.errors.load(Relaxed),
        start.elapsed()
    );
}
