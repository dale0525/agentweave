use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SkillStoreIdentity {
    managed: PathBuf,
    staging: PathBuf,
    quarantine: PathBuf,
}

impl SkillStoreIdentity {
    pub(crate) fn new(managed: PathBuf, staging: PathBuf, quarantine: PathBuf) -> Self {
        Self {
            managed,
            staging,
            quarantine,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RevisionLockKey {
    store: SkillStoreIdentity,
    revision_id: String,
}

type RevisionLockMap = HashMap<RevisionLockKey, Weak<AsyncMutex<()>>>;

fn revision_locks() -> &'static Mutex<RevisionLockMap> {
    static LOCKS: OnceLock<Mutex<RevisionLockMap>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) async fn acquire_revision_lock(
    store: &SkillStoreIdentity,
    revision_id: &str,
) -> OwnedMutexGuard<()> {
    let key = RevisionLockKey {
        store: store.clone(),
        revision_id: revision_id.to_string(),
    };
    let lock = {
        let mut locks = revision_locks()
            .lock()
            .expect("revision lock registry poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(&key).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = std::sync::Arc::new(AsyncMutex::new(()));
                locks.insert(key, std::sync::Arc::downgrade(&lock));
                lock
            }
        }
    };
    lock.lock_owned().await
}
