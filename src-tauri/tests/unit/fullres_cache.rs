use super::*;
use crate::subprocess::StagedOutput;

struct Cache {
    _directory: tempfile::TempDir,
    root: PathBuf,
    owner: Arc<Owner>,
}

impl Cache {
    fn new(budget: u64, max_entries: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("fullres");
        let owner = owner(&root).unwrap();
        {
            let mut state = owner.state.lock().unwrap();
            state.budget = budget;
            state.max_entries = max_entries;
        }
        Self { _directory: directory, root, owner }
    }

    fn target(&self, hash: &str) -> PathBuf {
        let path = self.root.join(&hash[..1]).join(format!("{hash}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        path
    }

    fn begin(&self, target: &Path, bytes: u64) -> Arc<Producer> {
        match begin(&self.root, target, Some(bytes)).unwrap() {
            Admission::Produce(producer, limit) => {
                assert_eq!(limit, bytes);
                producer
            }
            Admission::Existing => panic!("expected a new render reservation"),
        }
    }

    fn stage(&self, target: &Path, producer: &Arc<Producer>, bytes: usize) -> Arc<StagedOutput> {
        let stage = Arc::new(StagedOutput::with_owner(
            target.with_extension("tmp"),
            Some(producer.clone()),
        ).unwrap());
        std::fs::write(stage.path(), vec![7; bytes]).unwrap();
        stage
    }

    fn render(&self, hash: &str, bytes: usize) -> PathBuf {
        let target = self.target(hash);
        let producer = self.begin(&target, bytes as u64);
        let stage = self.stage(&target, &producer, bytes);
        producer.publish(stage.path()).unwrap();
        target
    }
}

#[test]
fn successive_renders_evict_the_least_recently_accessed_inactive_entry() {
    let cache = Cache::new(12, 3);
    let first = cache.render("aaa", 4);
    let second = cache.render("bbb", 4);
    let third = cache.render("ccc", 4);
    drop(read(&cache.root, &first).unwrap());

    let fourth = cache.render("ddd", 4);
    assert!(!second.exists());
    assert!(first.exists());
    assert!(third.exists());
    assert!(fourth.exists());

    let active = read(&cache.root, &third).unwrap();
    let fifth = cache.render("eee", 4);
    assert!(!first.exists());
    assert!(third.exists());
    assert!(fourth.exists());
    assert!(fifth.exists());
    drop(active);
    let state = cache.owner.state.lock().unwrap();
    assert_eq!(state.entries.len(), 3);
    assert_eq!(state.entries.values().map(|entry| entry.bytes).sum::<u64>(), 12);
}

#[test]
fn entry_capacity_evicts_even_when_the_byte_budget_has_room() {
    let cache = Cache::new(100, 2);
    let first = cache.render("aaa", 4);
    let second = cache.render("bbb", 4);
    assert!(matches!(begin(&cache.root, &first, Some(4)).unwrap(), Admission::Existing));
    let third = cache.render("ccc", 4);
    assert!(first.exists());
    assert!(!second.exists());
    assert!(third.exists());
}

#[test]
fn an_old_producer_drop_cannot_remove_a_new_reservation_for_the_same_path() {
    let cache = Cache::new(8, 1);
    let target = cache.target("aaa");
    let old_producer = cache.begin(&target, 4);
    let old_stage = cache.stage(&target, &old_producer, 4);
    old_producer.publish(old_stage.path()).unwrap();
    drop(old_stage);

    let other = cache.render("bbb", 4);
    assert!(!target.exists());
    assert!(other.exists());
    let new_producer = cache.begin(&target, 4);
    assert!(!other.exists());
    drop(old_producer);
    {
        let state = cache.owner.state.lock().unwrap();
        let entry = state.entries.get(&target).unwrap();
        assert!(entry.producing);
        assert_eq!(entry.bytes, 4);
    }
    let stage = cache.stage(&target, &new_producer, 4);
    new_producer.publish(stage.path()).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), vec![7; 4]);
}

#[test]
fn reader_prevents_eviction_and_retirement_waits_for_its_last_lease() {
    let cache = Cache::new(8, 2);
    let first = cache.render("aaa", 4);
    let second = cache.render("bbb", 4);
    let reader = read(&cache.root, &first).unwrap();
    let retained_reader = reader.clone();
    let third = cache.render("ccc", 4);
    assert!(first.exists());
    assert!(!second.exists());
    assert!(third.exists());

    retire(&first);
    assert!(first.exists());
    assert_eq!(std::fs::read(&first).unwrap(), vec![7; 4]);
    assert!(read(&cache.root, &first).is_err());
    drop(reader);
    assert!(first.exists());
    drop(retained_reader);
    assert!(!first.exists());
    assert!(!cache.owner.state.lock().unwrap().entries.contains_key(&first));
}

#[test]
fn staging_keeps_the_producer_reservation_until_its_last_owner_drops() {
    let cache = Cache::new(8, 2);
    let target = cache.target("aaa");
    let producer = cache.begin(&target, 8);
    let weak_producer = Arc::downgrade(&producer);
    let stage = cache.stage(&target, &producer, 3);
    let stage_path = stage.path().to_path_buf();
    let retained_stage = stage.clone();
    drop(producer);
    assert!(weak_producer.upgrade().is_some());
    {
        let state = cache.owner.state.lock().unwrap();
        let entry = state.entries.get(&target).unwrap();
        assert!(entry.producing);
        assert_eq!(entry.bytes, 8);
    }
    drop(stage);
    assert!(stage_path.exists());
    assert!(weak_producer.upgrade().is_some());
    drop(retained_stage);
    assert!(!stage_path.exists());
    assert!(weak_producer.upgrade().is_none());
    assert!(!cache.owner.state.lock().unwrap().entries.contains_key(&target));
}

#[test]
fn clear_retires_an_active_producer_without_removing_its_staging_file() {
    let cache = Cache::new(8, 2);
    let target = cache.target("aaa");
    let producer = cache.begin(&target, 8);
    let stage = cache.stage(&target, &producer, 4);
    let stage_path = stage.path().to_path_buf();
    let retained_stage = stage.clone();

    clear(&cache.root).unwrap();
    assert!(cache.owner.state.lock().unwrap().entries.get(&target).unwrap().retired);
    assert!(stage_path.exists());
    assert_eq!(producer.publish(stage.path()).unwrap_err(), crate::scanner::CANCELLED);
    assert!(!target.exists());
    drop(producer);
    drop(stage);
    assert!(stage_path.exists());
    assert!(cache.owner.state.lock().unwrap().entries.contains_key(&target));
    drop(retained_stage);
    assert!(!stage_path.exists());
    assert!(!cache.owner.state.lock().unwrap().entries.contains_key(&target));
}

#[test]
fn publication_refuses_bytes_beyond_the_reservation_and_cleans_the_partial() {
    let cache = Cache::new(8, 2);
    let target = cache.target("aaa");
    let producer = cache.begin(&target, 4);
    let stage = cache.stage(&target, &producer, 5);
    let stage_path = stage.path().to_path_buf();
    let error = producer.publish(stage.path()).unwrap_err();
    assert!(error.starts_with(crate::resource_limits::SAFETY_ERROR_PREFIX));
    assert!(!target.exists());
    assert!(stage_path.exists());
    drop(producer);
    assert!(cache.owner.state.lock().unwrap().entries.contains_key(&target));
    drop(stage);
    assert!(!stage_path.exists());
    assert!(!cache.owner.state.lock().unwrap().entries.contains_key(&target));
}

#[test]
fn provisional_cleanup_preserves_real_hashes_and_active_readers() {
    let cache = Cache::new(16, 4);
    let real = cache.render("a012345", 4);
    let inactive_provisional = cache.render("p1", 4);
    let active_provisional = cache.render("p2", 4);
    let reader = read(&cache.root, &active_provisional).unwrap();

    clear_where(&cache.root, true).unwrap();
    assert!(real.exists());
    assert!(matches!(begin(&cache.root, &real, Some(4)).unwrap(), Admission::Existing));
    assert!(!inactive_provisional.exists());
    assert!(active_provisional.exists());
    assert!(read(&cache.root, &active_provisional).is_err());
    assert_eq!(std::fs::read(&active_provisional).unwrap(), vec![7; 4]);
    drop(reader);
    assert!(!active_provisional.exists());
    assert!(real.exists());
    let state = cache.owner.state.lock().unwrap();
    assert_eq!(state.entries.len(), 1);
    assert!(state.entries.contains_key(&real));
}

#[test]
fn exhausted_storage_refuses_before_creating_a_render_or_reservation() {
    let cache = Cache::new(0, 2);
    let target = cache.target("aaa");
    let error = match begin(&cache.root, &target, Some(4)) {
        Err(error) => error,
        Ok(_) => panic!("zero storage must not admit a render"),
    };
    assert!(error.starts_with(crate::resource_limits::SAFETY_ERROR_PREFIX));
    assert!(!target.exists());
    assert!(cache.owner.state.lock().unwrap().entries.is_empty());
}
