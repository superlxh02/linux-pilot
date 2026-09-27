//! 有界本地批次缓冲。先持久化再发送，收到服务端 ACK 后删除。
//!
//! 文件名为固定宽度序号，因此字典序等于发送顺序。`sequence` 水位单独落盘，
//! 避免最后一个待发文件被 ACK 删除后，重启复用旧序号。异常批次移动到
//! `rejected/`，保留排障证据同时让后续正常批次继续前进。

use anyhow::Context;
use linux_pilot_model::MetricBatch;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// 一个宿主机身份对应一个缓冲目录；该目录应挂载到持久卷。
pub struct Spool {
    directory: PathBuf,
    stream_id: uuid::Uuid,
    sequence: u64,
    dropped: u64,
}

impl Spool {
    /// 打开缓冲目录，并从序号文件与待发文件中恢复最大的序号。
    ///
    /// 两个来源取最大值是崩溃恢复的关键：进程可能在批次文件落盘后、
    /// 水位文件提交前退出，也可能在全部批次被确认后退出。
    pub fn new(directory: PathBuf) -> anyhow::Result<Self> {
        fs::create_dir_all(&directory)?;
        // 宿主内核可能多年不重启，但容器的缓冲卷可能被重建。
        // 去重身份必须同时包含独立的缓冲实例 ID；否则卷丢失后序号
        // 从 1 开始，中心端会把新数据误认为同一内核启动期的旧批次。
        let stream_path = directory.join("stream-id");
        let stream_id = match fs::read_to_string(&stream_path)
            .ok()
            .and_then(|text| uuid::Uuid::parse_str(text.trim()).ok())
        {
            Some(id) => id,
            None => {
                let id = uuid::Uuid::new_v4();
                durable_replace(
                    &directory.join("stream-id.tmp"),
                    &stream_path,
                    id.to_string().as_bytes(),
                )?;
                id
            }
        };
        let pending_max = fs::read_dir(&directory)?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.path().file_stem()?.to_str()?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        // ACK 删除最后一个批次后仍需保留递增序号，否则同一 boot_id 会再次使用旧序号。
        let saved = fs::read_to_string(directory.join("sequence"))
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let sequence = pending_max.max(saved);
        Ok(Self {
            directory,
            stream_id,
            sequence,
            dropped: 0,
        })
    }

    /// 和真实内核 boot ID 共同形成一个传输 epoch；同一卷重启保持稳定，
    /// 换卷即使仍处于同一次内核启动，也不会复用数据库去重键。
    pub fn boot_stream_id(&self, kernel_boot_id: &str) -> String {
        format!("{kernel_boot_id}:{}", self.stream_id)
    }

    /// 分配同一 Worker 进程内严格递增的序号，供批次去重键使用。
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    /// 原子写入 JSON 批次与序号水位，然后实施 10,000 批次容量上限。
    ///
    /// 超限时删除最旧批次并累计丢弃计数；这是有意的有限缓冲策略，
    /// 可防止长期断网耗尽宿主机磁盘。
    pub fn store(&mut self, batch: &MetricBatch) -> anyhow::Result<()> {
        let path = self.directory.join(format!("{:020}.json", batch.sequence));
        let temporary = path.with_extension("tmp");
        // 每一步都同步到磁盘。崩溃发生在 rename 前时只留下 tmp，重启后不会回放半个批次。
        durable_replace(&temporary, &path, &serde_json::to_vec(batch)?)?;
        // 批次文件先落盘，再保存水位。若此处崩溃，重启时仍能从 pending 最大序号恢复。
        durable_replace(
            &self.directory.join("sequence.tmp"),
            &self.directory.join("sequence"),
            batch.sequence.to_string().as_bytes(),
        )?;
        // 最大保留 10,000 秒的批次；溢出时优先丢弃最旧的批次。
        let pending = self.files()?;
        for old in pending.iter().take(pending.len().saturating_sub(10_000)) {
            fs::remove_file(old)?;
            self.dropped += 1;
        }
        Ok(())
    }

    /// 返回待发批次数、占用字节和本进程累计丢弃批次，用于自观测。
    pub fn stats(&self) -> anyhow::Result<(usize, u64, u64)> {
        let files = self.files()?;
        let bytes = files
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum();
        Ok((files.len(), bytes, self.dropped))
    }

    /// 只列出路径，不预先解析所有批次；断线积压再多也只需保存少量文件名。
    pub fn pending_paths(&self) -> anyhow::Result<Vec<PathBuf>> {
        self.files()
    }

    /// 按需读取一个批次；文件可能在列目录后收到 ACK 而消失，属于正常并发。
    pub fn read_batch(&self, path: &Path) -> anyhow::Result<Option<MetricBatch>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("读取缓冲失败: {}", path.display()));
            }
        };
        let sequence = path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse::<u64>().ok());
        match serde_json::from_slice::<MetricBatch>(&bytes) {
            Ok(batch) if sequence == Some(batch.sequence) => Ok(Some(batch)),
            Ok(_) => {
                tracing::warn!(path = %path.display(), "批次文件名与序号不一致，已隔离");
                self.quarantine(path)?;
                Ok(None)
            }
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "本地批次损坏，已隔离");
                self.quarantine(path)?;
                Ok(None)
            }
        }
    }

    /// ACK 后删除对应批次；重复 ACK 是安全的幂等操作。
    pub fn ack(&self, sequence: u64) -> anyhow::Result<()> {
        let path = self.directory.join(format!("{sequence:020}.json"));
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// 将服务端明确拒绝的批次移入隔离目录，保留证据并防止它阻塞后续上报。
    pub fn reject(&self, sequence: u64) -> anyhow::Result<()> {
        let source = self.directory.join(format!("{sequence:020}.json"));
        if !source.exists() {
            return Ok(());
        }
        self.quarantine(&source)
    }

    fn quarantine(&self, source: &Path) -> anyhow::Result<()> {
        let quarantine = self.directory.join("rejected");
        fs::create_dir_all(&quarantine)?;
        let filename = source.file_name().context("批次文件名无效")?;
        fs::rename(source, quarantine.join(filename))?;
        Ok(())
    }

    fn files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut files: Vec<_> = fs::read_dir(&self.directory)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        files.sort();
        Ok(files)
    }
}

/// 通过同目录临时文件和原子替换提交数据，保证断电后的缓冲文件可解析。
fn durable_replace(temporary: &Path, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = fs::File::create(temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_batch_and_sequence_survive_restart() {
        let dir = std::env::temp_dir().join(format!("po-spool-test-{}", uuid::Uuid::new_v4()));
        let mut spool = Spool::new(dir.clone()).unwrap();
        let first_identity = spool.boot_stream_id("kernel-boot");
        let batch = MetricBatch {
            host_id: "h".into(),
            boot_id: "b".into(),
            sequence: spool.next_sequence(),
            metrics: vec![],
        };
        spool.store(&batch).unwrap();
        drop(spool);
        let mut restarted = Spool::new(dir.clone()).unwrap();
        assert_eq!(restarted.boot_stream_id("kernel-boot"), first_identity);
        let paths = restarted.pending_paths().unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(
            restarted.read_batch(&paths[0]).unwrap().unwrap().sequence,
            1
        );
        restarted.ack(1).unwrap();
        assert_eq!(restarted.next_sequence(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn new_spool_directory_gets_new_dedup_identity() {
        let first = std::env::temp_dir().join(format!("po-stream-a-{}", uuid::Uuid::new_v4()));
        let second = std::env::temp_dir().join(format!("po-stream-b-{}", uuid::Uuid::new_v4()));
        let a = Spool::new(first.clone()).unwrap();
        let b = Spool::new(second.clone()).unwrap();
        assert_ne!(
            a.boot_stream_id("same-kernel"),
            b.boot_stream_id("same-kernel")
        );
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }
}
