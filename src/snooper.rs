//! 嗅探器生命周期管理
//!
//! - `obugs --on`    → 启动嗅探器（前台，PTY 包装当前终端）
//! - `obugs --off`   → 发送 SIGTERM 停止
//! - `obugs --status` → 查询状态

use anyhow::{Context, Result};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

const STATE_DIR: &str = "obugs";
const PID_FILE: &str = "obugs.pid";

fn state_dir() -> Result<PathBuf> {
    let base = dirs::state_dir().or_else(|| {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".local").join("state"))
    }).context("cannot determine state directory")?;
    let dir = base.join(STATE_DIR);
    fs::create_dir_all(&dir).context("create state dir")?;
    Ok(dir)
}

fn pid_path() -> Result<PathBuf> {
    Ok(state_dir()?.join(PID_FILE))
}

/// 启动测试模式（前台阻塞）
/// 包装当前终端，输出 10 组随机位置的 ScreenBuffer 字符到 stderr。
pub fn start_test_snooper() -> Result<()> {
    let my_pid = std::process::id();
    eprintln!("🐛 oh-bugs! 测试模式已启动 (PID {my_pid})");
    eprintln!("   不触发虫子，仅输出随机位置的 ScreenBuffer 字符");
    eprintln!();

    let result = crate::daemon::run_test();
    result
}

/// 启动嗅探器（前台，PTY 包装当前终端）
pub fn start_snooper() -> Result<()> {
    let pid_path = pid_path()?;

    // 检查是否已在运行
    if let Ok(Some(pid)) = read_pid() {
        if is_pid_alive(pid as libc::pid_t) {
            eprintln!("🐛 oh-bugs! 嗅探器已在运行 (PID {pid})");
            eprintln!("   使用 `obugs --off` 停止它");
            return Ok(());
        } else {
            let _ = fs::remove_file(&pid_path);
        }
    }

    // 写入 PID
    let my_pid = std::process::id();
    fs::write(&pid_path, my_pid.to_string()).context("write pid file")?;

    // 设置 SIGTERM 处理
    setup_signal_handler()?;

    eprintln!("🐛 oh-bugs! 嗅探器已启动 (PID {my_pid})");
    eprintln!("   当前终端已包装 - 有 error 时自动放虫子");
    eprintln!("   停止: `obugs --off`（从另一个终端执行）");
    eprintln!();

    // 运行主循环（阻塞）
    let result = crate::daemon::run();

    // 清理 PID
    let _ = fs::remove_file(&pid_path);

    result
}

/// 停止嗅探器
pub fn stop_snooper() -> Result<()> {
    let pid_path = pid_path()?;

    let pid = match read_pid()? {
        Some(pid) => pid,
        None => {
            eprintln!("🐛 oh-bugs! 嗅探器未在运行");
            return Ok(());
        }
    };

    if !is_pid_alive(pid as libc::pid_t) {
        eprintln!("🐛 oh-bugs! 嗅探器 (PID {pid}) 已停止");
        let _ = fs::remove_file(&pid_path);
        return Ok(());
    }

    let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if ret != 0 {
        eprintln!("⚠️  无法停止嗅探器: {}", std::io::Error::last_os_error());
        return Ok(());
    }

    let mut waited = false;
    for _ in 0..40 {
        if !is_pid_alive(pid as libc::pid_t) {
            waited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    if waited {
        eprintln!("🐛 oh-bugs! 嗅探器已停止");
    } else {
        eprintln!("⚠️  嗅探器未响应，尝试强制终止...");
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    }

    let _ = fs::remove_file(&pid_path);
    Ok(())
}

/// 显示状态
pub fn show_status() -> Result<()> {
    match read_pid()? {
        None => {
            eprintln!("🐛 oh-bugs! 嗅探器状态: 未运行");
        }
        Some(pid) => {
            if is_pid_alive(pid as libc::pid_t) {
                eprintln!("🐛 oh-bugs! 嗅探器状态: **运行中**");
                eprintln!("   PID: {pid}");
                if let Some(uptime) = get_uptime(pid) {
                    let h = uptime / 3600;
                    let m = (uptime % 3600) / 60;
                    let s = uptime % 60;
                    eprintln!("   运行时间: {h}时{m}分{s}秒");
                }
            } else {
                eprintln!("🐛 oh-bugs! 嗅探器状态: 已停止");
                let _ = fs::remove_file(&pid_path()?);
            }
        }
    }
    Ok(())
}

// ─── 内部 ───

fn read_pid() -> Result<Option<u32>> {
    let path = pid_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).context("read pid")?;
    let pid: u32 = content.trim().parse().context("parse pid")?;
    Ok(Some(pid))
}

fn is_pid_alive(pid: libc::pid_t) -> bool {
    if pid <= 0 {
        return false;
    }
    unsafe { libc::kill(pid, 0) == 0 }
}

fn setup_signal_handler() -> Result<()> {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handle_sigterm as *const () as libc::sighandler_t;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = libc::SA_SIGINFO;
        if libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut()) != 0 {
            anyhow::bail!("sigaction: {}", std::io::Error::last_os_error());
        }
    }
    Ok(())
}

extern "C" fn handle_sigterm(_sig: libc::c_int) {
    crate::daemon::request_shutdown();
}

fn get_uptime(pid: u32) -> Option<u64> {
    let stat_path = format!("/proc/{pid}/stat");
    let mut f = fs::File::open(&stat_path).ok()?;
    let mut c = String::new();
    f.read_to_string(&mut c).ok()?;
    let boot = {
        let mut sf = fs::File::open("/proc/stat").ok()?;
        let mut d = String::new();
        sf.read_to_string(&mut d).ok()?;
        d.lines().find(|l| l.starts_with("btime "))
            .and_then(|l| l.trim_start_matches("btime ").trim().parse::<u64>().ok())?
    };
    let ticks: u64 = c.split_whitespace().nth(21)?.parse().ok()?;
    let start = boot + ticks / 100;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some(now.saturating_sub(start))
}
