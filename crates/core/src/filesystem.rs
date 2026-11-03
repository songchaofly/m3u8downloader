use crate::{AppError, Result};
use std::path::{Path, PathBuf};

fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Reject redirected application-owned paths before reading, writing or removing them.
pub fn check_tree(root: &Path, path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if linked(&metadata) || !std::fs::canonicalize(path)?.starts_with(root) {
        return Err(AppError::new(
            "PERMISSION_DENIED",
            "缓存包含目录联接或外部链接，操作已停止",
        ));
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            check_tree(root, &entry?.path())?;
        }
    }
    Ok(())
}

pub fn task_cache(root: &Path, id: &str) -> Result<PathBuf> {
    uuid::Uuid::parse_str(id).map_err(|_| AppError::new("INPUT_INVALID", "任务 ID 无效"))?;
    let root = std::fs::canonicalize(root)?;
    let path = root.join(id);
    match std::fs::create_dir(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    check_tree(&root, &path)?;
    if !path.is_dir() {
        return Err(AppError::new("PERMISSION_DENIED", "缓存路径不是目录"));
    }
    Ok(path)
}

/// Replaces an app-owned file without opening or truncating the old destination.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
