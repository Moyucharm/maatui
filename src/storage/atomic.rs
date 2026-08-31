use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) fn atomic_write(path: &Path, trusted_root: &Path, content: &[u8]) -> Result<()> {
    let trusted_root = canonicalize_trusted_root(trusted_root)?;
    validate_existing_ancestors(path, &trusted_root)?;
    let save_path = canonicalize_for_write(path)?;
    if !save_path.starts_with(&trusted_root) {
        bail!(
            "拒绝写入受控目录外的路径: {}（受控根: {}）",
            path.display(),
            trusted_root.display()
        );
    }

    let permissions = fs::metadata(&save_path)
        .ok()
        .map(|metadata| metadata.permissions());
    let (temp_path, mut temp) = create_temp_file(&save_path)?;
    let write_result = (|| -> Result<()> {
        if let Some(permissions) = permissions {
            fs::set_permissions(&temp_path, permissions)?;
        }
        temp.write_all(content)?;
        temp.sync_all()?;
        drop(temp);
        fs::rename(&temp_path, &save_path)
            .with_context(|| format!("替换数据文件失败: {}", save_path.display()))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn canonicalize_trusted_root(root: &Path) -> Result<PathBuf> {
    let root =
        fs::canonicalize(root).with_context(|| format!("解析受控目录失败: {}", root.display()))?;
    if !fs::metadata(&root)
        .with_context(|| format!("读取受控目录失败: {}", root.display()))?
        .is_dir()
    {
        bail!("受控根不是目录: {}", root.display());
    }
    Ok(root)
}

fn validate_existing_ancestors(path: &Path, trusted_root: &Path) -> Result<()> {
    let mut current = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    loop {
        match fs::symlink_metadata(&current) {
            Ok(_) => {
                let resolved = fs::canonicalize(&current)
                    .with_context(|| format!("解析写入路径失败: {}", path.display()))?;
                if !resolved.starts_with(trusted_root) {
                    bail!(
                        "拒绝通过符号链接离开受控目录: {} -> {}",
                        current.display(),
                        resolved.display()
                    );
                }
                if resolved == trusted_root {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("读取写入路径元信息失败: {}", path.display()));
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    Ok(())
}

/// 解析写入路径，同时保留最终目标文件的符号链接语义。
///
/// 对不存在的文件，会解析最近存在的父目录并把缺失部分接回去；
/// 这样父目录中的符号链接也会参与受控根边界检查。
fn canonicalize_for_write(path: &Path) -> Result<PathBuf> {
    let mut current = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let mut missing = Vec::<OsString>::new();

    loop {
        match fs::symlink_metadata(current) {
            Ok(_) => {
                let mut resolved = fs::canonicalize(current)
                    .with_context(|| format!("解析写入路径失败: {}", path.display()))?;
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let file_name = current
                    .file_name()
                    .with_context(|| format!("写入路径无效: {}", path.display()))?;
                missing.push(file_name.to_os_string());
                current = current
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("读取写入路径元信息失败: {}", path.display()));
            }
        }
    }
}

fn create_temp_file(path: &Path) -> Result<(PathBuf, File)> {
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("数据文件名无效")?;
    for _ in 0..100 {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path = path.with_file_name(format!(
            ".{file_name}.maatui.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("创建临时数据失败: {}", temp_path.display()));
            }
        }
    }
    bail!("无法创建唯一的临时数据文件")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn writes_new_file_atomically() {
        let dir = temp_dir("maatui-atomic");
        let path = dir.join("nested/data.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        atomic_write(&path, &dir, b"first\n").unwrap();
        atomic_write(&path, &dir, b"second\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preserves_symlink_to_internal_target() {
        let dir = temp_dir("maatui-atomic-internal");
        let real_dir = dir.join("real");
        fs::create_dir_all(&real_dir).unwrap();
        let path = dir.join("linked");
        symlink(&real_dir, &path).unwrap();

        atomic_write(&path.join("data.json"), &dir, b"new").unwrap();

        assert_eq!(
            fs::read_to_string(real_dir.join("data.json")).unwrap(),
            "new"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_parent_symlink_outside_trusted_root() {
        let dir = temp_dir("maatui-atomic-root");
        let outside = temp_dir("maatui-atomic-outside");
        fs::write(outside.join("data.json"), "old").unwrap();
        let linked_dir = dir.join("linked");
        symlink(&outside, &linked_dir).unwrap();

        assert!(atomic_write(&linked_dir.join("data.json"), &dir, b"new").is_err());
        assert_eq!(
            fs::read_to_string(outside.join("data.json")).unwrap(),
            "old"
        );
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn rejects_parent_symlink_that_returns_inside_trusted_root() {
        let dir = temp_dir("maatui-atomic-roundtrip-root");
        let outside = temp_dir("maatui-atomic-roundtrip-outside");
        let real_dir = dir.join("real");
        fs::create_dir_all(&real_dir).unwrap();
        symlink(&real_dir, outside.join("return")).unwrap();
        let linked_dir = dir.join("linked");
        symlink(&outside, &linked_dir).unwrap();

        assert!(atomic_write(&linked_dir.join("return/data.json"), &dir, b"new").is_err());
        assert!(!real_dir.join("data.json").exists());
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
    #[test]
    fn allows_parent_symlink_inside_trusted_root() {
        let dir = temp_dir("maatui-atomic-internal-root");
        let real_dir = dir.join("real");
        fs::create_dir_all(&real_dir).unwrap();
        let linked_dir = dir.join("linked");
        symlink(&real_dir, &linked_dir).unwrap();

        atomic_write(&linked_dir.join("data.json"), &dir, b"new").unwrap();

        assert_eq!(
            fs::read_to_string(real_dir.join("data.json")).unwrap(),
            "new"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_symlink_to_external_target() {
        let dir = temp_dir("maatui-atomic-target");
        let outside = temp_dir("maatui-atomic-target-outside");
        let target = outside.join("target.json");
        let path = dir.join("data.json");
        fs::write(&target, "old").unwrap();
        symlink(&target, &path).unwrap();

        assert!(atomic_write(&path, &dir, b"new").is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
