//! Session render ownership: completed bytes, producer reservations and physical
//! readers share one budget. Only inactive entries can be evicted or retired.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Entry {
    generation: u64,
    bytes: u64,
    used: u64,
    readers: usize,
    producing: bool,
    retired: bool,
}
struct State {
    entries: HashMap<PathBuf, Entry>,
    clock: u64,
    budget: u64,
    max_entries: usize,
}
struct Owner { state: Mutex<State>, changed: Condvar }
static OWNERS: OnceLock<Mutex<HashMap<PathBuf, Arc<Owner>>>> = OnceLock::new();

fn safety(message: &str) -> String {
    format!("{}{message}", crate::resource_limits::SAFETY_ERROR_PREFIX)
}

fn owner(root: &Path) -> Result<Arc<Owner>, String> {
    let owners = OWNERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut owners = owners.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(owner) = owners.get(root) { return Ok(owner.clone()); }
    std::fs::create_dir_all(root).map_err(|e| crate::resource_limits::cache_write_error("full-resolution cache", e))?; // data root
    // Five percent of available storage keeps this disposable session cache
    // subordinate to authored files. Metadata also has a host-memory bound.
    let budget = crate::resource_limits::available_storage_bytes(root)? / 20;
    let max_entries = (crate::resource_limits::available_memory_bytes().unwrap_or(0) / (1024 * 1024)).max(1) as usize;
    let mut state = State { entries: HashMap::new(), clock: 0, budget, max_entries };
    // Startup normally clears this directory before requests. Adopt surviving
    // entries conservatively; retain no library-sized inventory in memory.
    for entry in walkdir::WalkDir::new(root).follow_links(false) { // data root
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "png") { continue; }
        let bytes = entry.metadata().map_err(|e| e.to_string())?.len(); // data root
        if bytes > budget { crate::fs_recovery::remove_file(entry.path(), "oversize full-resolution cache cleanup"); continue; }
        make_room(&mut state, bytes, 1)?;
        state.clock = state.clock.saturating_add(1);
        state.entries.insert(entry.path().to_path_buf(), Entry { bytes, used: state.clock, ..Entry::default() });
    }
    let owner = Arc::new(Owner { state: Mutex::new(state), changed: Condvar::new() });
    owners.insert(root.to_path_buf(), owner.clone());
    Ok(owner)
}

fn remove(state: &mut State, path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) { // data root
        Ok(()) => {},
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(crate::resource_limits::cache_write_error("full-resolution cache eviction", e)),
    }
    state.entries.remove(path);
    Ok(())
}

fn make_room(state: &mut State, bytes: u64, entries: usize) -> Result<bool, String> {
    loop {
        let used = state.entries.values().fold(0u64, |sum, e| sum.saturating_add(e.bytes));
        if used.saturating_add(bytes) <= state.budget && state.entries.len().saturating_add(entries) <= state.max_entries { return Ok(true); }
        let victim = state.entries.iter().filter(|(_, e)| !e.producing && e.readers == 0)
            .min_by_key(|(_, e)| e.used).map(|(path, _)| path.clone());
        let Some(victim) = victim else { return Ok(false); };
        remove(state, &victim)?;
    }
}

pub(crate) enum Admission { Existing, Produce(Arc<Producer>, u64) }
pub(crate) struct Producer { owner: Arc<Owner>, target: PathBuf, generation: u64 }

pub(crate) fn begin(root: &Path, target: &Path, expected: Option<u64>) -> Result<Admission, String> {
    let owner = owner(root)?;
    let started = Instant::now();
    let mut state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        if crate::derived_runtime::cancelled() { return Err(crate::scanner::CANCELLED.to_string()); }
        state.clock = state.clock.saturating_add(1);
        let clock = state.clock;
        if let Some(entry) = state.entries.get_mut(target) {
            if !entry.producing && !entry.retired && target.is_file() { // data root
                entry.used = clock;
                return Ok(Admission::Existing);
            }
            if !entry.producing && entry.readers == 0 { remove(&mut state, target)?; }
        }
        if !state.entries.contains_key(target) {
            let reserved = state.entries.values().filter(|e| e.producing).fold(0u64, |sum, e| sum.saturating_add(e.bytes));
            let available = (crate::resource_limits::available_storage_bytes(root)? / 2).saturating_sub(reserved);
            let limit = expected.unwrap_or(state.budget).min(state.budget).min(available);
            if limit == 0 { return Err(safety("No storage budget is available for original-pixel inspection")); }
            if make_room(&mut state, limit, 1)? {
                state.entries.insert(target.to_path_buf(), Entry { generation: clock, bytes: limit, used: clock, producing: true, ..Entry::default() });
                drop(state);
                return Ok(Admission::Produce(Arc::new(Producer { owner, target: target.to_path_buf(), generation: clock }), limit));
            }
        }
        if started.elapsed() >= Duration::from_secs(2) { return Err(safety("Original-pixel cache capacity is still in use; retry after the active work settles")); }
        state = owner.changed.wait_timeout(state, Duration::from_millis(100)).unwrap_or_else(|p| p.into_inner()).0;
    }
}

impl Producer {
    pub(crate) fn publish(&self, staged: &Path) -> Result<(), String> {
        let mut state = self.owner.state.lock().unwrap_or_else(|p| p.into_inner());
        let entry = state.entries.get_mut(&self.target).ok_or_else(|| crate::scanner::CANCELLED.to_string())?;
        if entry.generation != self.generation || entry.retired || !entry.producing { return Err(crate::scanner::CANCELLED.to_string()); }
        let bytes = std::fs::metadata(staged).map_err(|e| e.to_string())?.len(); // data root
        if bytes > entry.bytes { return Err(safety("Original-pixel render exceeded its storage reservation")); }
        std::fs::rename(staged, &self.target).map_err(|e| crate::resource_limits::cache_write_error("full-resolution publication", e))?; // data root
        entry.bytes = bytes;
        entry.producing = false;
        self.owner.changed.notify_all();
        Ok(())
    }
}
impl Drop for Producer {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.entries.get(&self.target).is_some_and(|e| e.generation == self.generation && (e.producing || e.retired) && e.readers == 0) {
            if let Some(entry) = state.entries.get_mut(&self.target) { entry.producing = false; entry.retired = true; }
            if let Err(error) = remove(&mut state, &self.target) { crate::logging::warn("full-resolution retirement failed", serde_json::json!({"error": error})); }
        }
        self.owner.changed.notify_all();
    }
}

pub(crate) struct Reader { owner: Arc<Owner>, target: PathBuf }
pub(crate) fn read(root: &Path, target: &Path) -> Result<Arc<Reader>, String> {
    let owner = owner(root)?;
    let mut state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
    state.clock = state.clock.saturating_add(1);
    let clock = state.clock;
    let entry = state.entries.get_mut(target).ok_or("render is no longer cached")?;
    if entry.retired || entry.producing { return Err("render is unavailable".into()); }
    entry.readers += 1;
    entry.used = clock;
    drop(state);
    Ok(Arc::new(Reader { owner, target: target.to_path_buf() }))
}
impl Drop for Reader {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = state.entries.get_mut(&self.target) {
            entry.readers = entry.readers.saturating_sub(1);
            if entry.retired && entry.readers == 0 && !entry.producing {
                if let Err(error) = remove(&mut state, &self.target) { crate::logging::warn("full-resolution retirement failed", serde_json::json!({"error": error})); }
            }
        }
        self.owner.changed.notify_all();
    }
}

pub(crate) fn retire(path: &Path) {
    let Some(root) = path.parent().and_then(Path::parent) else { return; };
    let owner = OWNERS.get().and_then(|owners| owners.lock().unwrap_or_else(|p| p.into_inner()).get(root).cloned());
    if let Some(owner) = owner {
        let mut state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = state.entries.get_mut(path) {
            entry.retired = true;
            if entry.readers != 0 || entry.producing { return; }
        }
        if let Err(error) = remove(&mut state, path) { crate::logging::warn("full-resolution retirement failed", serde_json::json!({"error": error})); }
    } else { crate::fs_recovery::remove_file(path, "full-resolution cache cleanup"); }
}

pub(crate) fn clear(root: &Path) -> Result<(), String> { clear_where(root, false) }

pub(crate) fn clear_where(root: &Path, provisional_only: bool) -> Result<(), String> {
    let owners = OWNERS.get_or_init(|| Mutex::new(HashMap::new()));
    let owners = owners.lock().unwrap_or_else(|p| p.into_inner());
    let owner = owners.get(root).cloned();
    if owner.is_none() && !provisional_only {
        // With no registered owner, startup precedes every producer/reader.
        // Keep owner creation excluded until the crash-debris removal settles.
        return match std::fs::remove_dir_all(root) { // data root
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(crate::resource_limits::cache_write_error("full-resolution cleanup", e)),
        };
    }
    let matches = |path: &Path| !provisional_only || path.file_stem().and_then(|s| s.to_str())
        .is_some_and(crate::scanner::is_provisional);
    if let Some(owner) = owner {
        drop(owners);
        let mut state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
        let keys: Vec<_> = state.entries.keys().filter(|path| matches(path)).cloned().collect();
        for path in keys {
            let entry = state.entries.get_mut(&path).unwrap();
            entry.retired = true;
            if !entry.producing && entry.readers == 0 { remove(&mut state, &path)?; }
        }
        // Registered producers own their staging files until actual settlement.
        // Do not scan/delete staging paths using a snapshot of producer state.
        owner.changed.notify_all();
        return Ok(());
    }
    // No owner exists; keep creation excluded throughout provisional cleanup.
    for entry in walkdir::WalkDir::new(root).follow_links(false) { // data root
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) if e.io_error().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => continue,
            Err(e) => return Err(e.to_string()),
        };
        if entry.file_type().is_file() && matches(entry.path()) {
            crate::fs_recovery::remove_file(entry.path(), "full-resolution provisional cleanup");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/fullres_cache.rs"]
mod tests;
