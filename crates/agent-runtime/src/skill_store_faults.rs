use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StoreFaultPoint {
    StagingCopyFile,
    StagingAuthorFile,
    StagingAuthorAfterReservation,
    StagingAuthorAfterSnapshot,
    StagingAuthorAfterRecord,
    IncomingCopyFile,
    QuarantineCopyFile,
    PromoteStagingRename,
    PromoteIncomingRename,
    PromoteDatabase,
    PromoteRestoreRename,
    QuarantineIncomingRename,
    QuarantineSourceRename,
    QuarantineDatabase,
    QuarantineRestoreRename,
    WriteAfterLock,
    PromoteAfterLock,
    QuarantineAfterLock,
    RevisionLockAttempt,
    CopyBeforeFileOpen,
    WriteBeforeTempOpen,
    WriteBeforeMetadataCommit,
    WriteBeforeRename,
    WriteAfterRenameMode,
    WriteAfterRenameRevalidate,
    WriteTempCleanup,
    WriteCandidateCleanup,
    #[cfg(test)]
    WriteRestore,
    #[cfg(test)]
    WriteIsolationRestore,
    ManagedDiscoveryTransientIo,
    ExecutionAfterSnapshot,
    ExecutionCopyFile,
    PromoteBeforeDestinationCommit,
    ManagedReadonly,
    ManagedReadonlyBeforeApply,
    PromoteDestinationCleanup,
    PromoteDestinationCleanupAfter,
    PromoteSourceCleanup,
    PromoteSourceCleanupBeforeApply,
    PromoteSourceCleanupAfter,
    ActivationAfterPrepare,
    ActivationAfterCandidateBuild,
    ActivationAfterCompensation,
    ActivationBeforeDurableCommit,
    ActivationAfterDurableCommit,
    ActivationAfterMemoryPublish,
    ActivationAfterEvent,
    ActivationAfterSourceCleanup,
    ActivationRequestAfterCommit,
    ActivationRequestBeforeCommit,
    DraftTestBeforeSnapshot,
    DraftTestBeforePreview,
    DraftTestBeforePersist,
    DraftTestAfterPersist,
    ValidateDraftBeforePersist,
    ImportAfterReserve,
    ImportAfterCopy,
    ImportBeforeRow,
    ImportAfterRow,
    ImportBeforeFinalize,
    ImportTerminal,
    TransferCleanup,
    QuarantineDestinationCleanup,
    QuarantineDestinationCleanupAfter,
    QuarantineSourceCleanup,
    QuarantineSourceCleanupAfter,
}

#[derive(Clone, Default)]
pub(crate) struct StoreFaults {
    failures: Arc<Mutex<BTreeMap<StoreFaultPoint, usize>>>,
    repeated_failures: Arc<Mutex<BTreeMap<StoreFaultPoint, usize>>>,
    gates: Arc<Mutex<BTreeMap<StoreFaultPoint, Arc<StoreTestGateInner>>>>,
    revision_id: Arc<Mutex<Option<String>>>,
}

#[derive(Debug)]
struct StoreTestGateInner {
    entered: tokio::sync::Barrier,
    release: tokio::sync::Barrier,
    has_entered: AtomicBool,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct StoreTestGate {
    inner: Arc<StoreTestGateInner>,
}

#[cfg(test)]
impl StoreTestGate {
    pub(crate) async fn wait_entered(&self) {
        self.inner.entered.wait().await;
    }

    pub(crate) async fn release(&self) {
        self.inner.release.wait().await;
    }

    pub(crate) fn has_entered(&self) -> bool {
        self.inner.has_entered.load(Ordering::Acquire)
    }
}

impl StoreFaults {
    #[cfg(test)]
    pub(crate) fn set_revision_id_once(&self, revision_id: &str) {
        *self.revision_id.lock().unwrap() = Some(revision_id.to_string());
    }

    pub(crate) fn take_revision_id(&self) -> Option<String> {
        self.revision_id.lock().unwrap().take()
    }

    #[cfg(test)]
    pub(crate) fn fail_once(&self, point: StoreFaultPoint) {
        self.failures.lock().unwrap().insert(point, 0);
    }

    #[cfg(test)]
    pub(crate) fn fail_after(&self, point: StoreFaultPoint, successful_checks: usize) {
        self.failures
            .lock()
            .unwrap()
            .insert(point, successful_checks);
    }

    #[cfg(test)]
    pub(crate) fn fail_times(&self, point: StoreFaultPoint, times: usize) {
        assert!(times > 0, "failure count must be positive");
        self.repeated_failures.lock().unwrap().insert(point, times);
    }

    #[cfg(test)]
    pub(crate) fn gate_once(&self, point: StoreFaultPoint) -> StoreTestGate {
        let inner = Arc::new(StoreTestGateInner {
            entered: tokio::sync::Barrier::new(2),
            release: tokio::sync::Barrier::new(2),
            has_entered: AtomicBool::new(false),
        });
        self.gates.lock().unwrap().insert(point, inner.clone());
        StoreTestGate { inner }
    }

    pub(crate) fn check(&self, point: StoreFaultPoint) -> anyhow::Result<()> {
        let mut repeated = self.repeated_failures.lock().unwrap();
        if let Some(remaining) = repeated.get_mut(&point) {
            *remaining -= 1;
            if *remaining == 0 {
                repeated.remove(&point);
            }
            anyhow::bail!("injected skill store failure at {point:?}")
        }
        drop(repeated);
        let mut failures = self.failures.lock().unwrap();
        let Some(remaining) = failures.get_mut(&point) else {
            return Ok(());
        };
        if *remaining == 0 {
            failures.remove(&point);
            anyhow::bail!("injected skill store failure at {point:?}")
        }
        *remaining -= 1;
        Ok(())
    }

    pub(crate) async fn checkpoint(&self, point: StoreFaultPoint) {
        let gate = self.gates.lock().unwrap().remove(&point);
        if let Some(gate) = gate {
            gate.has_entered.store(true, Ordering::Release);
            gate.entered.wait().await;
            gate.release.wait().await;
        }
    }
}
