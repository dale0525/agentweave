#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
fn transient_directory_open() -> &'static Mutex<Option<PathBuf>> {
    static TARGET: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    TARGET.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
pub(crate) fn inject_transient_directory_open_once(root: &Path) {
    *transient_directory_open().lock().unwrap() = Some(root.to_path_buf());
}

#[cfg(unix)]
pub(crate) fn check_directory_open(_root: &std::path::Path) -> Result<(), rustix::io::Errno> {
    #[cfg(test)]
    {
        let mut target = transient_directory_open().lock().unwrap();
        if target.as_deref() == Some(_root) {
            target.take();
            return Err(rustix::io::Errno::MFILE);
        }
    }
    Ok(())
}
