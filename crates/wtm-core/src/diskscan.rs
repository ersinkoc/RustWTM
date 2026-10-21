//! Where the space on a disk went: a folder tree with sizes, the largest
//! files and the space per file type, like TreeSize or WinDirStat.
//!
//! Sizes are logical file sizes (what Explorer shows as "Size"), not the
//! space allocated on disk. Symbolic links and junctions are not followed,
//! so nothing is counted twice and loops are impossible.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

/// One folder and everything below it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirNode {
    pub name: String,
    /// Bytes in this folder and all folders below it.
    pub size: u64,
    /// Files in this folder and all folders below it.
    pub files: u64,
    /// Bytes of the files directly in this folder.
    pub own_size: u64,
    /// Files directly in this folder.
    pub own_files: u64,
    /// Subfolders, largest first.
    pub children: Vec<DirNode>,
    /// The folder could not be listed (access denied, vanished).
    pub denied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileType {
    /// Lower-case extension without the dot; empty for files without one.
    pub extension: String,
    pub size: u64,
    pub files: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    pub root_path: PathBuf,
    pub root: DirNode,
    /// Largest files, largest first.
    pub largest: Vec<BigFile>,
    /// File types by total size, largest first.
    pub types: Vec<FileType>,
    pub folders: u64,
    /// Folders that could not be listed.
    pub denied: u64,
    pub elapsed: Duration,
    /// Stopped early; the numbers cover what was scanned.
    pub cancelled: bool,
}

/// Live counters of a running scan, shared with the window.
#[derive(Debug, Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub folders: AtomicU64,
    pub bytes: AtomicU64,
    cancel: AtomicBool,
    current: Mutex<String>,
}

impl Progress {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The folder being scanned right now.
    pub fn current(&self) -> String {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_current(&self, path: &Path) {
        if let Ok(mut c) = self.current.try_lock() {
            use std::fmt::Write;
            c.clear();
            let _ = write!(c, "{}", path.display());
        }
    }
}

/// Largest files kept by default.
pub const LARGEST_KEPT: usize = 200;
/// File types kept, largest first; the rest are summed into "other".
const TYPES_KEPT: usize = 40;

struct Walk<'a> {
    progress: &'a Progress,
    largest: BinaryHeap<Reverse<(u64, PathBuf, Option<SystemTime>)>>,
    keep: usize,
    types: HashMap<String, (u64, u64)>,
    denied: u64,
    /// Scratch for the lower-cased extension of the current file.
    ext: String,
}

/// Worker threads at most.
const WORKERS: usize = 4;

/// A folder still to list: its node id and its parent's.
struct Task {
    id: usize,
    parent: usize,
    path: PathBuf,
}

/// Folders waiting to be listed, shared by the workers. Any worker takes any
/// folder at any depth, so one huge folder (Users, Windows) is spread over
/// all of them instead of keeping a single worker busy to the end.
struct Queue {
    tasks: Vec<Task>,
    /// Workers listing a folder right now; they may still add more.
    busy: usize,
}

struct Shared<'a> {
    queue: Mutex<Queue>,
    wake: Condvar,
    next_id: AtomicUsize,
    progress: &'a Progress,
}

impl Shared<'_> {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next folder to list; `None` once all are listed or on cancel.
    fn take(&self) -> Option<Task> {
        let mut q = self.lock();
        loop {
            if self.progress.cancelled() {
                return None;
            }
            if let Some(task) = q.tasks.pop() {
                q.busy += 1;
                return Some(task);
            }
            if q.busy == 0 {
                return None;
            }
            q = self.wake.wait(q).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Marks a taken folder as done (also if listing it panicked, so the other
/// workers never wait forever), queueing its subfolders.
struct Listing<'s, 'a> {
    shared: &'s Shared<'a>,
    subdirs: Vec<Task>,
}

impl Drop for Listing<'_, '_> {
    fn drop(&mut self) {
        let mut q = self.shared.lock();
        q.busy -= 1;
        q.tasks.append(&mut self.subdirs);
        drop(q);
        self.shared.wake.notify_all();
    }
}

/// A listed folder: node id, parent id, and the node without its children.
type Found = (usize, usize, DirNode);

/// Lists folders until none are left, in background mode when `background`.
fn work<'a>(shared: &Shared<'a>, keep: usize, background: bool) -> (Walk<'a>, Vec<Found>) {
    if background {
        let _ = crate::control::enter_background_mode();
    }
    let mut w = Walk::new(shared.progress, keep);
    let mut found = Vec::new();
    while let Some(task) = shared.take() {
        let mut listing = Listing {
            shared,
            subdirs: Vec::new(),
        };
        let (node, subdirs) = w.list(&task.path);
        let first = shared.next_id.fetch_add(subdirs.len(), Ordering::Relaxed);
        // Reversed so the stack hands them out in listing order.
        listing.subdirs = subdirs
            .into_iter()
            .enumerate()
            .rev()
            .map(|(i, path)| Task {
                id: first + i,
                parent: task.id,
                path,
            })
            .collect();
        found.push((task.id, task.parent, node));
    }
    (w, found)
}

/// Builds the tree from the listed folders: `ids` ids were handed out, and
/// every child has a larger id than its parent, so going from the last id
/// down finishes each folder before it is added to its parent. Folders whose
/// parent is missing (a worker panicked) are left out.
fn assemble(found: Vec<Found>, ids: usize) -> Option<DirNode> {
    let mut slots: Vec<Option<(usize, DirNode)>> = (0..ids).map(|_| None).collect();
    for (id, parent, node) in found {
        slots[id] = Some((parent, node));
    }
    for id in (1..ids).rev() {
        let Some((parent, mut node)) = slots[id].take() else {
            continue;
        };
        sort_children(&mut node);
        if let Some((_, p)) = slots.get_mut(parent).and_then(Option::as_mut) {
            p.size += node.size;
            p.files += node.files;
            p.children.push(node);
        }
    }
    let (_, mut root) = slots.first_mut()?.take()?;
    sort_children(&mut root);
    Some(root)
}

/// Largest first, then by name.
fn sort_children(node: &mut DirNode) {
    node.children
        .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    node.children.shrink_to_fit();
}

/// Scans `root` and everything below it. Check `progress` from another
/// thread; [`Progress::cancel`] stops the scan soon after. The folders are
/// listed by a few worker threads in background mode (lowest CPU and I/O
/// priority), so the scan never slows the machine down.
pub fn scan(root: &Path, progress: &Progress, keep_largest: usize) -> ScanResult {
    let started = Instant::now();
    let shared = Shared {
        queue: Mutex::new(Queue {
            tasks: vec![Task {
                id: 0,
                parent: usize::MAX,
                path: root.to_owned(),
            }],
            busy: 0,
        }),
        wake: Condvar::new(),
        next_id: AtomicUsize::new(1),
        progress,
    };
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, WORKERS);
    let mut walk = Walk::new(progress, keep_largest);
    let mut found = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .filter_map(|_| {
                std::thread::Builder::new()
                    .name("disk-scan-worker".into())
                    .spawn_scoped(s, || work(&shared, keep_largest, true))
                    .ok()
            })
            .collect();
        if handles.is_empty() {
            let (w, f) = work(&shared, keep_largest, false);
            walk.absorb(w);
            found.extend(f);
        }
        for handle in handles {
            if let Ok((w, f)) = handle.join() {
                walk.absorb(w);
                found.extend(f);
            }
        }
    });
    // Without a listing of the root (cancelled first) the tree is empty.
    let mut node = assemble(found, shared.next_id.into_inner()).unwrap_or_default();
    node.name = root.display().to_string();

    let mut largest: Vec<BigFile> = walk
        .largest
        .into_iter()
        .map(|Reverse((size, path, modified))| BigFile {
            path,
            size,
            modified,
        })
        .collect();
    largest.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    let mut types: Vec<FileType> = walk
        .types
        .into_iter()
        .map(|(extension, (size, files))| FileType {
            extension,
            size,
            files,
        })
        .collect();
    types.sort_by(|a, b| {
        b.size
            .cmp(&a.size)
            .then_with(|| a.extension.cmp(&b.extension))
    });
    if types.len() > TYPES_KEPT {
        let rest = types.split_off(TYPES_KEPT - 1);
        types.push(FileType {
            extension: "*".into(),
            size: rest.iter().map(|t| t.size).sum(),
            files: rest.iter().map(|t| t.files).sum(),
        });
    }
    ScanResult {
        root_path: root.to_owned(),
        root: node,
        largest,
        types,
        folders: progress.folders.load(Ordering::Relaxed),
        denied: walk.denied,
        elapsed: started.elapsed(),
        cancelled: progress.cancelled(),
    }
}

impl<'a> Walk<'a> {
    fn new(progress: &'a Progress, keep: usize) -> Self {
        Self {
            progress,
            largest: BinaryHeap::new(),
            keep,
            types: HashMap::new(),
            denied: 0,
            ext: String::new(),
        }
    }

    /// Takes over what another walker found.
    fn absorb(&mut self, other: Walk<'_>) {
        self.denied += other.denied;
        for (ext, (size, files)) in other.types {
            let t = self.types.entry(ext).or_default();
            t.0 += size;
            t.1 += files;
        }
        for Reverse(item) in other.largest {
            self.largest.push(Reverse(item));
            if self.largest.len() > self.keep {
                self.largest.pop();
            }
        }
    }

    /// The files directly in `path`, and its subfolders still to scan.
    fn list(&mut self, path: &Path) -> (DirNode, Vec<PathBuf>) {
        let mut node = DirNode {
            name: path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            ..DirNode::default()
        };
        self.progress.folders.fetch_add(1, Ordering::Relaxed);
        self.progress.set_current(path);
        let Ok(entries) = std::fs::read_dir(path) else {
            node.denied = true;
            self.denied += 1;
            return (node, Vec::new());
        };
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            if self.progress.cancelled() {
                break;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // Junctions and symbolic links point elsewhere: not followed.
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                subdirs.push(entry.path());
                continue;
            }
            // On Windows this comes from the directory listing itself.
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let size = meta.len();
            node.own_size += size;
            node.own_files += 1;
            self.count_type(Path::new(&entry.file_name()).extension(), size);
            self.keep_if_large(size, &entry, &meta);
        }
        // Once per folder: per file, the workers would contend for the
        // counters' cache line.
        self.progress
            .files
            .fetch_add(node.own_files, Ordering::Relaxed);
        self.progress
            .bytes
            .fetch_add(node.own_size, Ordering::Relaxed);
        node.size = node.own_size;
        node.files = node.own_files;
        (node, subdirs)
    }

    /// Adds a file to its type, lower-cased into a reused buffer so the
    /// usual case (a type seen before) allocates nothing.
    fn count_type(&mut self, extension: Option<&std::ffi::OsStr>, size: u64) {
        let ext = extension.map(|e| e.to_string_lossy()).unwrap_or_default();
        self.ext.clear();
        if ext.is_ascii() {
            self.ext.push_str(&ext);
            self.ext.make_ascii_lowercase();
        } else {
            self.ext.push_str(&ext.to_lowercase());
        }
        match self.types.get_mut(self.ext.as_str()) {
            Some(t) => {
                t.0 += size;
                t.1 += 1;
            }
            None => {
                self.types.insert(self.ext.clone(), (size, 1));
            }
        }
    }

    fn keep_if_large(&mut self, size: u64, entry: &std::fs::DirEntry, meta: &std::fs::Metadata) {
        if self.keep == 0 {
            return;
        }
        if self.largest.len() >= self.keep {
            match self.largest.peek() {
                Some(Reverse((smallest, ..))) if size > *smallest => {
                    self.largest.pop();
                }
                _ => return,
            }
        }
        self.largest
            .push(Reverse((size, entry.path(), meta.modified().ok())));
    }
}

impl DirNode {
    /// The node at `path` (child indices from this node).
    pub fn at(&self, path: &[usize]) -> Option<&DirNode> {
        path.iter().try_fold(self, |node, &i| node.children.get(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(test: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wtm-scan-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (file, size) in [
            ("a.txt", 10usize),
            ("big/video.MP4", 5000),
            ("big/clip.mp4", 3000),
            ("big/deep/notes.txt", 20),
            ("small/readme", 7),
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![0u8; size]).unwrap();
        }
        std::fs::create_dir_all(root.join("empty")).unwrap();
        root
    }

    #[test]
    fn sums_folders_ranks_files_and_types() {
        let root = tree("sums");
        let progress = Progress::default();
        let r = scan(&root, &progress, 2);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(r.root.size, 8037);
        assert_eq!(r.root.files, 5);
        assert_eq!((r.root.own_size, r.root.own_files), (10, 1));
        let names: Vec<_> = r.root.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["big", "small", "empty"], "largest first");
        let big = &r.root.children[0];
        assert_eq!((big.size, big.files, big.own_size), (8020, 3, 8000));
        assert_eq!(r.root.at(&[0, 0]).unwrap().name, "deep");
        assert!(r.root.at(&[5]).is_none());

        // Only the two largest files are kept, largest first.
        let largest: Vec<_> = r.largest.iter().map(|f| f.size).collect();
        assert_eq!(largest, [5000, 3000]);
        assert!(r.largest[0].path.ends_with("big/video.MP4"));
        assert!(r.largest[0].modified.is_some());

        // Extensions are case-insensitive; files without one share "".
        assert_eq!(r.types[0].extension, "mp4");
        assert_eq!((r.types[0].size, r.types[0].files), (8000, 2));
        assert!(
            r.types
                .iter()
                .any(|t| t.extension.is_empty() && t.size == 7)
        );

        assert_eq!(r.folders, 5);
        assert_eq!(progress.files.load(Ordering::Relaxed), 5);
        assert_eq!(progress.bytes.load(Ordering::Relaxed), 8037);
        assert!(!r.cancelled && r.denied == 0);
    }

    #[test]
    fn a_missing_root_is_reported_not_fatal() {
        let r = scan(
            Path::new(r"C:\no\such\wtm\folder"),
            &Progress::default(),
            10,
        );
        assert!(r.root.denied);
        assert_eq!((r.root.size, r.denied), (0, 1));
    }

    #[test]
    fn cancelling_stops_early() {
        let root = tree("cancel");
        let progress = Progress::default();
        progress.cancel();
        let r = scan(&root, &progress, 10);
        let _ = std::fs::remove_dir_all(&root);
        assert!(r.cancelled);
        assert_eq!(r.root.files, 0);
        assert!(r.root.children.is_empty());
    }

    #[test]
    fn junctions_are_not_followed() {
        let root = tree("junction");
        let link = root.join("loop");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&root)
            .output()
            .is_ok_and(|o| o.status.success());
        let r = scan(&root, &Progress::default(), 10);
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&root);
        if made {
            assert!(r.root.children.iter().all(|c| c.name != "loop"));
        }
        assert_eq!(r.root.size, 8037, "counted once");
    }

    /// Real-world speed on the system drive:
    /// `cargo test --release -p wtm-core scans_the_system_drive -- --ignored --nocapture`.
    #[test]
    #[ignore = "scans the whole system drive"]
    fn scans_the_system_drive() {
        let root = format!(
            r"{}\",
            std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
        );
        let r = scan(Path::new(&root), &Progress::default(), LARGEST_KEPT);
        println!(
            "{root}: {} files, {} folders ({} denied), {:.1} GiB in {:.1} s",
            r.root.files,
            r.folders,
            r.denied,
            r.root.size as f64 / f64::from(1u32 << 30),
            r.elapsed.as_secs_f64()
        );
        assert!(r.root.files > 1000);
    }

    /// A synthetic tree: `top` folders right below the root, the first one
    /// holding `heavy` times the others' share, each leaf with 40 files.
    fn synthetic(test: &str, top: usize, heavy: usize) -> (PathBuf, u64) {
        let root = std::env::temp_dir().join(format!("wtm-scan-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut files = 0;
        for t in 0..top {
            let leaves = if t == 0 { 50 * heavy } else { 50 };
            for l in 0..leaves {
                let dir = root
                    .join(format!("top{t}"))
                    .join(format!("sub{}", l % 7))
                    .join(format!("leaf{l}"));
                std::fs::create_dir_all(&dir).unwrap();
                for f in 0..40 {
                    let ext = ["txt", "dll", "PNG", "json", "rs"][f % 5];
                    std::fs::write(dir.join(format!("file{f}.{ext}")), [0u8; 16]).unwrap();
                    files += 1;
                }
            }
        }
        (root, files)
    }

    /// `cargo test --release -p wtm-core scan_speed -- --ignored --nocapture`
    #[test]
    #[ignore = "timing harness; writes thousands of files"]
    fn scan_speed() {
        for (label, top, heavy) in [("balanced", 8, 1), ("one big folder", 2, 8)] {
            let (root, files) = synthetic(&format!("speed-{top}"), top, heavy);
            let mut best = Duration::MAX;
            for _ in 0..5 {
                let r = scan(&root, &Progress::default(), LARGEST_KEPT);
                assert_eq!(r.root.files, files);
                best = best.min(r.elapsed);
            }
            let _ = std::fs::remove_dir_all(&root);
            println!("{label}: {files} files, best of 5 {best:?}");
        }
    }

    #[test]
    fn many_types_are_capped_with_the_rest_summed() {
        let root = std::env::temp_dir().join(format!("wtm-scan-types-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..TYPES_KEPT + 5 {
            std::fs::write(root.join(format!("f.e{i}")), vec![0u8; i + 1]).unwrap();
        }
        let r = scan(&root, &Progress::default(), 0);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(r.types.len(), TYPES_KEPT);
        let other = r.types.last().unwrap();
        assert_eq!(other.extension, "*");
        assert_eq!(other.files, 6);
        assert_eq!(
            r.types.iter().map(|t| t.files).sum::<u64>(),
            (TYPES_KEPT + 5) as u64
        );
        assert!(r.largest.is_empty());
    }
}
