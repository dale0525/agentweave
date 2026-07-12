use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const GATE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(crate) struct BundleInspectionGate {
    entered: Arc<tokio::sync::Barrier>,
    release: Arc<tokio::sync::Barrier>,
}

impl BundleInspectionGate {
    pub(crate) async fn wait_entered(&self) {
        wait(&self.entered, "bundle inspection entry").await;
    }

    pub(crate) async fn release(&self) {
        wait(&self.release, "bundle inspection release").await;
    }
}

pub(crate) fn gate_bundle_after_inspection(output_root: &Path) -> BundleInspectionGate {
    let gate = BundleInspectionGate {
        entered: Arc::new(tokio::sync::Barrier::new(2)),
        release: Arc::new(tokio::sync::Barrier::new(2)),
    };
    inspection_gates()
        .lock()
        .unwrap()
        .insert(canonical_test_path(output_root), gate.clone());
    gate
}

fn inspection_gates() -> &'static Mutex<BTreeMap<PathBuf, BundleInspectionGate>> {
    static GATES: OnceLock<Mutex<BTreeMap<PathBuf, BundleInspectionGate>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(super) async fn checkpoint_after_inspection(output_root: &Path) {
    let gate = inspection_gates()
        .lock()
        .unwrap()
        .remove(&canonical_test_path(output_root));
    if let Some(gate) = gate {
        wait(&gate.entered, "bundle inspection checkpoint entry").await;
        wait(&gate.release, "bundle inspection checkpoint release").await;
    }
}

#[derive(Clone)]
pub(crate) struct BundlePublishGate {
    generation: Arc<Mutex<Option<PathBuf>>>,
    entered: Arc<tokio::sync::Barrier>,
    release: Arc<tokio::sync::Barrier>,
}

impl BundlePublishGate {
    pub(crate) async fn wait_entered(&self) -> PathBuf {
        wait(&self.entered, "bundle publication entry").await;
        self.generation.lock().unwrap().clone().unwrap()
    }

    pub(crate) async fn release(&self) {
        wait(&self.release, "bundle publication release").await;
    }
}

pub(crate) fn gate_bundle_before_publish(output_root: &Path) -> BundlePublishGate {
    insert_publish_gate(output_root, publish_gates())
}

pub(crate) fn gate_bundle_after_final_validation(output_root: &Path) -> BundlePublishGate {
    insert_publish_gate(output_root, final_validation_gates())
}

fn insert_publish_gate(
    output_root: &Path,
    gates: &Mutex<BTreeMap<PathBuf, BundlePublishGate>>,
) -> BundlePublishGate {
    let gate = BundlePublishGate {
        generation: Arc::new(Mutex::new(None)),
        entered: Arc::new(tokio::sync::Barrier::new(2)),
        release: Arc::new(tokio::sync::Barrier::new(2)),
    };
    gates
        .lock()
        .unwrap()
        .insert(canonical_test_path(output_root), gate.clone());
    gate
}

fn publish_gates() -> &'static Mutex<BTreeMap<PathBuf, BundlePublishGate>> {
    static GATES: OnceLock<Mutex<BTreeMap<PathBuf, BundlePublishGate>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn final_validation_gates() -> &'static Mutex<BTreeMap<PathBuf, BundlePublishGate>> {
    static GATES: OnceLock<Mutex<BTreeMap<PathBuf, BundlePublishGate>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(super) async fn checkpoint_before_publish(output_root: &Path, generation: &Path) {
    checkpoint_publish_gate(output_root, generation, publish_gates(), "pre-publication").await;
}

pub(super) async fn checkpoint_after_final_validation(output_root: &Path, generation: &Path) {
    checkpoint_publish_gate(
        output_root,
        generation,
        final_validation_gates(),
        "final-validation",
    )
    .await;
}

async fn checkpoint_publish_gate(
    output_root: &Path,
    generation: &Path,
    gates: &Mutex<BTreeMap<PathBuf, BundlePublishGate>>,
    label: &str,
) {
    let gate = gates
        .lock()
        .unwrap()
        .remove(&canonical_test_path(output_root));
    if let Some(gate) = gate {
        *gate.generation.lock().unwrap() = Some(generation.to_path_buf());
        wait(&gate.entered, &format!("{label} checkpoint entry")).await;
        wait(&gate.release, &format!("{label} checkpoint release")).await;
    }
}

fn canonical_test_path(path: &Path) -> PathBuf {
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    std::fs::canonicalize(parent)
        .map(|parent| parent.join(path.file_name().unwrap_or_default()))
        .unwrap_or_else(|_| path.to_path_buf())
}

async fn wait(barrier: &tokio::sync::Barrier, label: &str) {
    tokio::time::timeout(GATE_TIMEOUT, barrier.wait())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {label}"));
}
