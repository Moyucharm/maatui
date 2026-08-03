//! MaaTUI 自有数据的路径解析与原子写入。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};

pub fn maa_config_dir() -> Result<PathBuf> {
    resolve_maa_config_dir(
        std::env::var_os("MAA_CONFIG_DIR"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

fn resolve_maa_config_dir(
    maa_config_dir: Option<std::ffi::OsString>,
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    if let Some(dir) = maa_config_dir {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = xdg_config_home {
        return Ok(PathBuf::from(dir).join("maa"));
    }
    let home = home.context("无法确定 HOME，且未设置 MAA_CONFIG_DIR/XDG_CONFIG_HOME")?;
    Ok(PathBuf::from(home).join(".config/maa"))
}

pub fn maa_data_dir() -> Result<PathBuf> {
    resolve_maa_data_dir(
        std::env::var_os("MAA_DATA_DIR"),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
    )
}

fn resolve_maa_data_dir(
    maa_data_dir: Option<std::ffi::OsString>,
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    if let Some(dir) = maa_data_dir {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = xdg_data_home {
        return Ok(PathBuf::from(dir).join("maa"));
    }
    let home = home.context("无法确定 HOME，且未设置 MAA_DATA_DIR/XDG_DATA_HOME")?;
    Ok(PathBuf::from(home).join(".local/share/maa"))
}

pub fn maa_hot_update_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("MAA_HOT_UPDATE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(maa_data_dir()?.join("MaaResource"))
}

pub fn maatui_cache_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(dir).join("maatui"));
    }
    let home = std::env::var_os("HOME").context("无法确定 HOME，且未设置 XDG_CACHE_HOME")?;
    Ok(PathBuf::from(home).join(".cache/maatui"))
}

pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let save_path = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::canonicalize(path)
            .with_context(|| format!("解析数据链接失败: {}", path.display()))?,
        Ok(_) => path.to_path_buf(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => {
            return Err(error).with_context(|| format!("读取数据元信息失败: {}", path.display()));
        }
    };
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
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("maatui-storage-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn writes_new_file_atomically() {
        let dir = temp_dir();
        let path = dir.join("nested/data.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        atomic_write(&path, b"first\n").unwrap();
        atomic_write(&path, b"second\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resolves_xdg_and_override_directories() {
        assert_eq!(
            resolve_maa_config_dir(None, Some("/xdg/config".into()), Some("/home/me".into()))
                .unwrap(),
            PathBuf::from("/xdg/config/maa")
        );
        assert_eq!(
            resolve_maa_data_dir(None, Some("/xdg/data".into()), Some("/home/me".into())).unwrap(),
            PathBuf::from("/xdg/data/maa")
        );
        assert_eq!(
            resolve_maa_config_dir(
                Some("/custom/config".into()),
                Some("/xdg/config".into()),
                None,
            )
            .unwrap(),
            PathBuf::from("/custom/config")
        );
        assert_eq!(
            resolve_maa_data_dir(Some("/custom/data".into()), Some("/xdg/data".into()), None,)
                .unwrap(),
            PathBuf::from("/custom/data")
        );
    }

    #[test]
    fn preserves_permissions_and_symlink() {
        let dir = temp_dir();
        let target = dir.join("target.json");
        let path = dir.join("data.json");
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &path).unwrap();

        atomic_write(&path, b"new").unwrap();

        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
