//! Copilot 批量运行的纯状态迁移。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchPosition {
    Running {
        current: usize,
        total: usize,
        index: usize,
    },
    Finishing {
        total: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchCompletion {
    pub index: usize,
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug)]
pub struct BatchExit {
    pub task_path: PathBuf,
    pub reconcile_indices: Vec<usize>,
    pub abort_error: Option<String>,
}

#[derive(Debug)]
pub struct CopilotBatchState {
    task_path: PathBuf,
    indices: Vec<usize>,
    completed: usize,
    abort_error: Option<String>,
}

impl CopilotBatchState {
    pub fn new(task_path: PathBuf, indices: Vec<usize>) -> Self {
        Self {
            task_path,
            indices,
            completed: 0,
            abort_error: None,
        }
    }

    pub fn position(&self) -> Option<BatchPosition> {
        let total = self.indices.len();
        if total == 0 {
            return None;
        }
        if self.completed >= total {
            return Some(BatchPosition::Finishing { total });
        }
        Some(BatchPosition::Running {
            current: self.completed + 1,
            total,
            index: self.indices[self.completed],
        })
    }

    pub fn current_completion(&self) -> Option<BatchCompletion> {
        Some(BatchCompletion {
            index: *self.indices.get(self.completed)?,
            completed: self.completed + 1,
            total: self.indices.len(),
        })
    }

    pub fn mark_success(&mut self) {
        self.completed = (self.completed + 1).min(self.indices.len());
    }

    pub fn set_abort_error(&mut self, message: String) {
        self.abort_error = Some(message);
    }

    pub fn task_path(&self) -> &Path {
        &self.task_path
    }

    /// 调用方汇总进程退出与核心诊断；内部中止也不能补记剩余项成功。
    pub fn finish(self, successful_exit: bool) -> BatchExit {
        let reconcile_indices = if successful_exit && self.abort_error.is_none() {
            self.indices[self.completed.min(self.indices.len())..].to_vec()
        } else {
            Vec::new()
        };
        BatchExit {
            task_path: self.task_path,
            reconcile_indices,
            abort_error: self.abort_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_progress_and_finishing_state() {
        let mut state = CopilotBatchState::new("batch.json".into(), vec![3, 5]);
        assert_eq!(
            state.position(),
            Some(BatchPosition::Running {
                current: 1,
                total: 2,
                index: 3,
            })
        );
        assert_eq!(
            state.current_completion(),
            Some(BatchCompletion {
                index: 3,
                completed: 1,
                total: 2,
            })
        );
        state.mark_success();
        state.mark_success();
        assert_eq!(
            state.position(),
            Some(BatchPosition::Finishing { total: 2 })
        );
    }

    #[test]
    fn reconciles_only_clean_successful_exit() {
        let mut state = CopilotBatchState::new("batch.json".into(), vec![1, 2, 3]);
        state.mark_success();
        assert_eq!(state.finish(true).reconcile_indices, vec![2, 3]);

        let state = CopilotBatchState::new("batch.json".into(), vec![1, 2]);
        assert!(state.finish(false).reconcile_indices.is_empty());
        let mut state = CopilotBatchState::new("batch.json".into(), vec![1, 2]);
        state.set_abort_error("进度监视失败".to_string());
        let exit = state.finish(true);
        assert!(exit.reconcile_indices.is_empty());
        assert_eq!(exit.abort_error.as_deref(), Some("进度监视失败"));
    }
}
