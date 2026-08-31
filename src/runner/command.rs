use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Daily,
    Copilot,
    Roguelike,
    Update,
}

#[derive(Debug, Clone)]
pub struct TaskCommand {
    pub kind: TaskKind,
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
    /// 单任务执行：记录选中的任务索引，用于构造进度集合。
    pub daily_single_index: Option<usize>,
    /// 运行结束后需要删除的临时文件（单任务执行生成）。
    pub cleanup: Option<PathBuf>,
}

impl TaskCommand {
    pub fn daily() -> Self {
        Self::maa(TaskKind::Daily, "每日任务", ["run", "daily", "-v"])
    }

    /// 单独执行 daily 配置中第 `index` 个任务：运行只含该任务的临时文件，
    /// 结束后删除 `cleanup` 指向的临时任务文件。
    pub fn daily_single(index: usize, task_name: &str, file_name: &str, cleanup: PathBuf) -> Self {
        let mut command = Self::maa(
            TaskKind::Daily,
            format!("单任务：{task_name}"),
            ["run", file_name, "-v"],
        );
        command.daily_single_index = Some(index);
        command.cleanup = Some(cleanup);
        command
    }

    pub fn copilot(args: Vec<String>) -> Self {
        let mut command = Self::maa(TaskKind::Copilot, "自动战斗", std::iter::empty::<&str>());
        command.args = args;
        command
    }

    pub fn copilot_batch(task_path: &str) -> Self {
        Self::maa(
            TaskKind::Copilot,
            "作业集自动战斗",
            ["run", task_path, "--batch", "-v"],
        )
    }

    pub fn roguelike(task_name: &str, cleanup: PathBuf) -> Self {
        let mut command = Self::maa(
            TaskKind::Roguelike,
            "自动肉鸽",
            ["run", task_name, "--batch", "-v"],
        );
        command.cleanup = Some(cleanup);
        command
    }

    pub fn resource_update() -> Self {
        Self::maa(
            TaskKind::Update,
            "资源热更新",
            ["hot-update", "--batch", "-v"],
        )
    }

    pub fn core_update() -> Self {
        Self::maa(
            TaskKind::Update,
            "Core 与基础资源更新",
            ["update", "--batch", "-v"],
        )
    }

    fn maa<I, S>(kind: TaskKind, label: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            kind,
            label: label.into(),
            program: "maa".to_string(),
            args: args.into_iter().map(Into::into).collect(),
            envs: vec![("MAA_LOG_PREFIX".to_string(), "Always".to_string())],
            daily_single_index: None,
            cleanup: None,
        }
    }

    pub fn tracks_daily_progress(&self) -> bool {
        self.kind == TaskKind::Daily
    }

    pub fn tracks_copilot_progress(&self) -> bool {
        self.kind == TaskKind::Copilot
    }

    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}
