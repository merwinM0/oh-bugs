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
///
/// 返回值: (shell 路径, shell 名称, 启动参数)
fn detect_shell() -> (String, String, Vec<String>) {
    let shell_path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let shell_name = Path::new(&shell_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("bash")
        .to_string();

    let args: Vec<String> = match shell_name.as_str() {
        // bash: 不加载 .bashrc，提供干净环境（用户可在 shell 内 source）
        "bash" => vec![],
        // zsh: 也不加载 .zshrc
        "zsh" => vec!["-f".into()],          // -f = --no-rcs
        // fish: --no-config 跳过所有配置文件
        "fish" => vec!["--no-config".into()],
        // 其他 shell: 无特殊参数
        _ => vec![],
    };

    (shell_path, shell_name, args)
}

/// Shell 进程管理器 — 基于 PTY（伪终端）实现
///
/// 自动适配用户默认 shell（bash / fish / zsh 等），
/// 通过 PTY 驱动提供完整的终端回显、提示符、任务控制。
pub struct Shell {
    pub shell_name: String,             // 标识当前 shell，供外部逻辑参考
    master: File,                       // PTY master — 读写 shell 数据
    child: Child,                       // 子进程句柄
    output_rx: mpsc::Receiver<Vec<u8>>,
}

impl Shell {
    /// 启动用户默认 shell，通过 PTY 连接
    pub fn spawn() -> Result<Self> {
        let (shell_path, shell_name, shell_args) = detect_shell();

        // ── 1. 打开 PTY master/slave 对 ──
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

                    // ① 成为 session leader
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }

                    // ② 将 PTY slave 设为此 session 的控制终端
                    if libc::ioctl(fd, libc::TIOCSCTTY, 0) < 0 {
                        let err = std::io::Error::last_os_error();
                        if err.raw_os_error() != Some(libc::EINVAL) {
                            return Err(err);
                        }
                    }

                    // ③ 设置 raw 模式
                    let mut termios: libc::termios = std::mem::zeroed();
                    if libc::tcgetattr(fd, &mut termios) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    libc::cfmakeraw(&mut termios);
                    if libc::tcsetattr(fd, libc::TCSAFLUSH, &termios) < 0 {
                        return Err(std::io::Error::last_os_error());
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

    /// 设置 PTY 的终端尺寸（行列数）
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

    /// 等待 shell 子进程退出
    #[allow(dead_code)]
    pub fn wait(&mut self) -> Result<std::process::ExitStatus> {
        Ok(self.child.wait()?)
    }
}
