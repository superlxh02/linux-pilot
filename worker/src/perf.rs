//! 按需 perf CPU 采样。所有命令参数独立传递，绝不执行拼接后的 shell 文本。

use linux_pilot_wire::agent::{ProfileCommand, ProfileResult};
use std::{collections::BTreeMap, sync::OnceLock, time::Duration};
use tokio::{process::Command, sync::Semaphore};

static PERF_AVAILABLE: OnceLock<bool> = OnceLock::new();
static PROFILE_LIMIT: Semaphore = Semaphore::const_new(1);

pub fn available() -> bool {
    *PERF_AVAILABLE.get_or_init(|| {
        std::process::Command::new("perf")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

/// 运行一次受限 CPU profile 并将采样栈折叠为火焰图输入格式。
pub async fn run(command: ProfileCommand, host_id: String) -> ProfileResult {
    let job_id = command.job_id.clone();
    match run_inner(&command).await {
        Ok((folded_stacks, sample_count)) => ProfileResult {
            job_id,
            host_id,
            success: true,
            error: String::new(),
            folded_stacks,
            sample_count,
        },
        Err(error) => ProfileResult {
            job_id,
            host_id,
            success: false,
            error: error.to_string(),
            folded_stacks: String::new(),
            sample_count: 0,
        },
    }
}

async fn run_inner(command: &ProfileCommand) -> anyhow::Result<(String, u64)> {
    if !available() {
        anyhow::bail!("宿主机未安装 perf");
    }
    if command.pid <= 0
        || !(1..=60).contains(&command.duration_s)
        || !(1..=199).contains(&command.frequency_hz)
    {
        anyhow::bail!("perf 参数超出允许范围");
    }
    // Worker 运行在宿主机 PID 命名空间。先检查目标是否存在，避免把输错
    // PID 或已经退出的进程误报为“没有热点”。这里不缓存检查结果：采样期间
    // 进程仍可能退出，后续的空样本错误会再次说明这种可能性。
    let proc_path = format!("/proc/{}/stat", command.pid);
    if !std::path::Path::new(&proc_path).exists() {
        anyhow::bail!(
            "目标 PID {} 不存在；请使用所选 Worker 节点上的进程 PID",
            command.pid
        );
    }
    // 每台主机仅允许一个高开销采样任务，避免影响被观测负载。
    let _permit = PROFILE_LIMIT.acquire().await?;
    let path = std::env::temp_dir().join(format!("po-perf-{}.data", uuid::Uuid::new_v4()));
    let output = tokio::time::timeout(
        Duration::from_secs(command.duration_s as u64 + 15),
        Command::new("perf")
            .kill_on_drop(true)
            .arg("record")
            .arg("-q")
            .arg("-F")
            .arg(command.frequency_hz.to_string())
            .arg("-g")
            .arg("--call-graph")
            .arg("fp")
            .arg("-p")
            .arg(command.pid.to_string())
            .arg("-o")
            .arg(&path)
            .arg("--")
            .arg("sleep")
            .arg(command.duration_s.to_string())
            .output(),
    )
    .await;
    let output = match output {
        Ok(result) => result?,
        Err(_) => {
            let _ = tokio::fs::remove_file(&path).await;
            anyhow::bail!("perf record 超时，已终止采样进程");
        }
    };
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let _ = tokio::fs::remove_file(&path).await;
        anyhow::bail!("perf record 失败: {}", message.trim());
    }
    let script = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new("perf")
            .kill_on_drop(true)
            .arg("script")
            .arg("-i")
            .arg(&path)
            .output(),
    )
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    let script = script.map_err(|_| anyhow::anyhow!("perf script 超时，已终止解析进程"))??;
    if !script.status.success() {
        anyhow::bail!(
            "perf script 失败: {}",
            String::from_utf8_lossy(&script.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&script.stdout);
    let mut stacks: BTreeMap<String, u64> = BTreeMap::new();
    for event in text.split("\n\n") {
        let mut frames = Vec::new();
        for line in event.lines().skip(1) {
            // perf script 默认栈行的第二列为符号。保留 unknown 标志，前端可显示符号化缺口。
            if let Some(symbol) = line.split_whitespace().nth(1) {
                frames.push(symbol.replace(';', ":"));
            }
        }
        if !frames.is_empty() {
            frames.reverse();
            *stacks.entry(frames.join(";")).or_default() += 1;
        }
    }
    let sample_count = stacks.values().sum();
    if sample_count == 0 {
        if text.trim().is_empty() {
            anyhow::bail!(
                "perf 已运行，但 PID {} 在 {} 秒内没有 CPU 采样事件；请选当前正在消耗 CPU 的进程，或延长采样时间",
                command.pid,
                command.duration_s
            );
        }
        anyhow::bail!(
            "perf 已采到 PID {} 的事件，但没有可解析的调用栈；请检查目标程序的符号与帧指针，或查看 Worker 的 perf 日志",
            command.pid
        );
    }
    let folded = stacks
        .into_iter()
        .map(|(stack, count)| format!("{stack} {count}"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok((folded, sample_count))
}
