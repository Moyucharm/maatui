//! Daily 配置后台保存队列。

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;

use crate::storage::atomic_write;

#[derive(Debug)]
pub struct SaveResult {
    pub revision: u64,
    pub result: Result<(), String>,
}

struct SaveRequest {
    revision: u64,
    path: PathBuf,
    trusted_root: PathBuf,
    content: Vec<u8>,
}

struct QueueState {
    pending: Option<SaveRequest>,
    closed: bool,
}

pub struct ConfigSaveWorker {
    queue: Arc<(Mutex<QueueState>, Condvar)>,
    results: mpsc::Receiver<SaveResult>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ConfigSaveWorker {
    pub fn new() -> Result<Self, String> {
        let queue = Arc::new((
            Mutex::new(QueueState {
                pending: None,
                closed: false,
            }),
            Condvar::new(),
        ));
        let (result_tx, result_rx) = mpsc::channel();
        let worker_queue = Arc::clone(&queue);
        let handle = thread::Builder::new()
            .name("maatui-config-save".to_string())
            .spawn(move || {
                loop {
                    let request = {
                        let (lock, wake) = &*worker_queue;
                        let mut state = lock.lock().expect("配置保存队列锁已中毒");
                        while state.pending.is_none() && !state.closed {
                            state = wake.wait(state).expect("配置保存队列锁已中毒");
                        }
                        if let Some(request) = state.pending.take() {
                            request
                        } else {
                            break;
                        }
                    };
                    let result =
                        atomic_write(&request.path, &request.trusted_root, &request.content)
                            .map_err(|error| error.to_string());
                    if result_tx
                        .send(SaveResult {
                            revision: request.revision,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| format!("无法启动配置保存线程: {error}"))?;
        Ok(Self {
            queue,
            results: result_rx,
            handle: Some(handle),
        })
    }

    pub fn enqueue(&self, revision: u64, path: PathBuf, trusted_root: PathBuf, content: Vec<u8>) {
        let (lock, wake) = &*self.queue;
        let mut state = lock.lock().expect("配置保存队列锁已中毒");
        if !state.closed {
            state.pending = Some(SaveRequest {
                revision,
                path,
                trusted_root,
                content,
            });
            wake.notify_one();
        }
    }

    /// 阻塞等待指定 revision 或更高 revision 的最终写入结果。
    ///
    /// 未开始的旧快照可能会被更新快照替换，因此只要更高 revision 成功，
    /// 就代表当前配置已经落盘；低于目标 revision 的结果会被消费但不会提前返回。
    pub fn flush_until(&self, revision: u64) -> Result<u64, String> {
        loop {
            match self.results.recv() {
                Ok(result) if result.revision < revision => continue,
                Ok(result) => return result.result.map(|()| result.revision),
                Err(_) => return Err("配置保存线程已退出，无法确认写入结果".to_string()),
            }
        }
    }

    pub fn poll(&self) -> Vec<SaveResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.results.try_recv() {
            results.push(result);
        }
        results
    }

    pub fn shutdown(&mut self) {
        let (lock, wake) = &*self.queue;
        if let Ok(mut state) = lock.lock() {
            state.closed = true;
            wake.notify_one();
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ConfigSaveWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn latest_pending_snapshot_wins() {
        let root = std::env::temp_dir().join(format!(
            "maatui-config-save-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("daily.json");
        let mut worker = ConfigSaveWorker::new().unwrap();
        worker.enqueue(1, path.clone(), root.clone(), b"old".to_vec());
        worker.enqueue(2, path.clone(), root.clone(), b"new".to_vec());
        assert_eq!(worker.flush_until(2).unwrap(), 2);
        worker.shutdown();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flush_reports_write_failure() {
        let root = std::env::temp_dir().join(format!(
            "maatui-config-save-failure-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let blocked = root.join("blocked");
        fs::create_dir_all(&blocked).unwrap();
        let mut worker = ConfigSaveWorker::new().unwrap();
        worker.enqueue(1, blocked.clone(), root.clone(), b"content".to_vec());
        assert!(worker.flush_until(1).is_err());
        worker.shutdown();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flush_ignores_old_failure_when_new_revision_succeeds() {
        let root = std::env::temp_dir().join(format!(
            "maatui-config-save-revisions-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let blocked = root.join("blocked");
        fs::create_dir_all(&blocked).unwrap();
        let path = root.join("daily.json");
        let mut worker = ConfigSaveWorker::new().unwrap();
        worker.enqueue(1, blocked, root.clone(), b"old".to_vec());
        worker.enqueue(2, path.clone(), root.clone(), b"new".to_vec());
        assert_eq!(worker.flush_until(2).unwrap(), 2);
        worker.shutdown();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        fs::remove_dir_all(root).unwrap();
    }
}
