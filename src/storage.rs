//! MaaTUI 自有数据的路径解析、缓存维护与原子写入。

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

mod atomic;
mod paths;

pub(crate) use atomic::atomic_write;
pub(crate) use paths::{
    maa_config_dir, maa_data_dir, maa_hot_update_dir, maa_log_dir, maatui_cache_dir,
};
#[cfg(test)]
use paths::{resolve_maa_config_dir, resolve_maa_data_dir, resolve_maa_state_dir};

pub(crate) fn clear_maa_avatar_cache() -> Result<usize> {
    clear_avatar_cache_in(&maa_config_dir()?.join("cache/avatars"))
}

fn clear_avatar_cache_in(dir: &Path) -> Result<usize> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| format!("读取干员头像缓存失败: {}", dir.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("干员头像缓存路径不是安全目录: {}", dir.display());
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| format!("读取干员头像缓存失败: {}", dir.display()));
        }
    };

    let mut removed = 0;
    for entry in entries {
        let entry = entry.with_context(|| format!("读取干员头像缓存失败: {}", dir.display()))?;
        if !entry
            .file_type()
            .with_context(|| format!("读取缓存文件类型失败: {}", entry.path().display()))?
            .is_file()
            || !entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        {
            continue;
        }
        fs::remove_file(entry.path()).with_context(|| {
            format!(
                "删除干员头像缓存失败: {}（此前已删除 {removed} 个）",
                entry.path().display()
            )
        })?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;
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
    fn clears_only_top_level_avatar_png_files() {
        let root = temp_dir();
        let avatars = root.join("cache/avatars");
        let nested = avatars.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(avatars.join("char_a.png"), "avatar").unwrap();
        fs::write(avatars.join("char_b.PNG"), "avatar").unwrap();
        fs::write(avatars.join("keep.json"), "metadata").unwrap();
        fs::write(nested.join("keep.png"), "nested avatar").unwrap();
        symlink(avatars.join("char_a.png"), avatars.join("linked.png")).unwrap();

        assert_eq!(clear_avatar_cache_in(&avatars).unwrap(), 2);
        assert!(!avatars.join("char_a.png").exists());
        assert!(!avatars.join("char_b.PNG").exists());
        assert!(avatars.join("keep.json").is_file());
        assert!(nested.join("keep.png").is_file());
        assert!(fs::symlink_metadata(avatars.join("linked.png")).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_empty_avatar_cache_needs_no_cleanup() {
        let root = temp_dir();
        let avatars = root.join("cache/avatars");
        assert_eq!(clear_avatar_cache_in(&avatars).unwrap(), 0);
        fs::create_dir_all(&avatars).unwrap();
        assert_eq!(clear_avatar_cache_in(&avatars).unwrap(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_symlinked_avatar_cache_directory() {
        let root = temp_dir();
        let outside = temp_dir();
        fs::write(outside.join("keep.png"), "avatar").unwrap();
        fs::create_dir_all(root.join("cache")).unwrap();
        let avatars = root.join("cache/avatars");
        symlink(&outside, &avatars).unwrap();

        assert!(clear_avatar_cache_in(&avatars).is_err());
        assert!(outside.join("keep.png").is_file());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn writes_new_file_atomically() {
        let dir = temp_dir();
        let path = dir.join("nested/data.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        atomic_write(&path, &dir, b"first\n").unwrap();
        atomic_write(&path, &dir, b"second\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resolves_state_directories() {
        assert_eq!(
            resolve_maa_state_dir(
                Some("/custom/state".into()),
                Some("/xdg/state".into()),
                None,
            )
            .unwrap(),
            PathBuf::from("/custom/state")
        );
        assert_eq!(
            resolve_maa_state_dir(None, Some("/xdg/state".into()), Some("/home/me".into()))
                .unwrap(),
            PathBuf::from("/xdg/state/maa")
        );
        assert_eq!(
            resolve_maa_state_dir(None, None, Some("/home/me".into())).unwrap(),
            PathBuf::from("/home/me/.local/state/maa")
        );
        assert_eq!(
            resolve_maa_state_dir(None, None, None)
                .unwrap_err()
                .to_string(),
            "无法确定 HOME，且未设置 MAA_STATE_DIR/XDG_STATE_HOME"
        );
    }

    #[test]
    fn resolves_config_and_data_directories() {
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

        atomic_write(&path, &dir, b"new").unwrap();

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

    #[test]
    fn rejects_symlink_to_external_target() {
        let dir = temp_dir();
        let outside = temp_dir();
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
