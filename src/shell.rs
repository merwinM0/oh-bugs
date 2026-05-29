//! PTY Shell 进程管理器
//!
//! 创建 PTY 伪终端对，在 slave 端启动用户的默认 Shell（bash/zsh/fish），
//! 通过 master fd 读写 Shell I/O。所有 shell 配置正常加载。

use anyhow::{Context, Result};
use nix::fcntl::OFlag;
use nix::pty::{grantpt, posix_openpt, ptsname, unlockpt};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// 检测用户的默认 shell
fn detect_shell() -> (String, String, Vec<String>) {
    let shell_path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let shell_name = Path::new(&shell_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("bash")
        .to_string();

    // 所有 Shell 正常加载配置（不传限制性参数）
    let args: Vec<String> = match shell_name.as_str() {
        _ => vec![],
    };

    (shell_path, shell_name, args)
}

/// Shell 进程管理器
pub struct Shell {
    pub shell_name: String,
    master: File,
    child: Child,
    output_rx: mpsc::Receiver<Vec<u8>>,
}

impl Shell {
    /// 启动用户默认 shell，通过 PTY 连接
    pub fn spawn() -> Result<Self> {
        let (shell_path, shell_name, shell_args) = detect_shell();

        // ── 1. 打开 PTY master/slave ──
        let master_fd = posix_openpt(OFlag::O_RDWR | OFlag::O_CLOEXEC)
            .context("posix_openpt")?;
        grantpt(&master_fd).context("grantpt")?;
        unlockpt(&master_fd).context("unlockpt")?;
        let slave_name = unsafe { ptsname(&master_fd) }.context("ptsname")?;

        let master_raw = master_fd.as_raw_fd();
        let reader_fd = unsafe { libc::dup(master_raw) };
        if reader_fd < 0 {
            anyhow::bail!("dup master: {}", std::io::Error::last_os_error());
        }
        let master = unsafe { File::from_raw_fd(master_raw) };
        std::mem::forget(master_fd);

        // ── 2. 打开 slave 设备 ──
        let slave_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(slave_name.as_str())
            .context("open slave pty")?;
        let slave_raw = slave_file.as_raw_fd();

        let slave_in = unsafe { libc::dup(slave_raw) };
        let slave_out = unsafe { libc::dup(slave_raw) };
        let slave_err = unsafe { libc::dup(slave_raw) };
        if slave_in < 0 || slave_out < 0 || slave_err < 0 {
            anyhow::bail!("dup slave: {}", std::io::Error::last_os_error());
        }
        drop(slave_file);

        // ── 3. 启动 shell，用 slave fd 作为终端 ──
        let child = unsafe {
            Command::new(&shell_path)
                .args(&shell_args)
                .stdin(Stdio::from_raw_fd(slave_in))
                .stdout(Stdio::from_raw_fd(slave_out))
                .stderr(Stdio::from_raw_fd(slave_err))
                .pre_exec(|| {
                    let fd = libc::STDIN_FILENO;

                    // 成为 session leader
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }

                    // 将 PTY slave 设为此 session 的控制终端
                    if libc::ioctl(fd, libc::TIOCSCTTY, 0) < 0 {
                        let err = std::io::Error::last_os_error();
                        if err.raw_os_error() != Some(libc::EINVAL) {
                            return Err(err);
                        }
                    }

                    Ok(())
                })
                .spawn()
                .context("spawn shell")?
        };

        // ── 4. 启动读取线程（从 PTY master 读取输出） ──
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = unsafe { File::from_raw_fd(reader_fd) };
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Shell {
            shell_name,
            master,
            child,
            output_rx: rx,
        })
    }

    /// 设置 PTY 的终端尺寸
    pub fn set_window_size(&self, cols: u16, rows: u16) -> Result<()> {
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let ret = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
        if ret != 0 {
            anyhow::bail!("ioctl TIOCSWINSZ: {}", std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// 将数据写入 PTY master（发送给 shell）
    pub fn write_input(&mut self, data: &[u8]) -> Result<()> {
        self.master.write_all(data)?;
        self.master.flush()?;
        Ok(())
    }

    /// 尝试从输出 channel 接收数据（非阻塞）
    pub fn try_recv_output(&mut self) -> Result<Vec<u8>, mpsc::TryRecvError> {
        self.output_rx.try_recv()
    }

    /// 检查 shell 子进程是否仍在运行
    pub fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            _ => false,
        }
    }
}
