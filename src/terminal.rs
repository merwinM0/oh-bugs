//! 终端控制封装
//!
//! 支持两种模式：
//! - stdout 模式：使用 crossterm 直接操作当前终端（适用于 PTY 包装模式）
//! - 设备文件模式：直接写入终端设备文件（如 /dev/pts/5），适用于 LD_PRELOAD 守护进程

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{self, size as terminal_size};
use std::io::{stdout, Write};

/// 终端管理器
pub struct Terminal {
    original_raw_mode: bool,
}

impl Terminal {
    /// 进入原始模式，隐藏光标
    #[allow(dead_code)]
    pub fn enter() -> anyhow::Result<Self> {
        let original_raw_mode = terminal::is_raw_mode_enabled()?;
        if !original_raw_mode {
            terminal::enable_raw_mode()?;
        }
        let mut stdout = stdout();
        execute!(stdout, Hide)?;
        stdout.flush()?;
        Ok(Self { original_raw_mode })
    }

    /// 恢复终端状态
    pub fn restore(&self) -> anyhow::Result<()> {
        let mut stdout = stdout();
        execute!(stdout, Show)?;
        stdout.flush()?;
        if !self.original_raw_mode {
            terminal::disable_raw_mode()?;
        }
        Ok(())
    }

    /// 获取当前终端大小（列，行）
    #[allow(dead_code)]
    pub fn size() -> anyhow::Result<(u16, u16)> {
        Ok(terminal_size()?)
    }

    /// 获取终端设备文件的大小（通过 TIOCGWINSZ ioctl）
    pub fn device_size(dev: &std::fs::File) -> anyhow::Result<(u16, u16)> {
        use std::os::fd::AsRawFd;
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::ioctl(dev.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
        if ret != 0 {
            anyhow::bail!("ioctl TIOCGWINSZ: {}", std::io::Error::last_os_error());
        }
        Ok((ws.ws_col, ws.ws_row))
    }

    /// 在指定位置写入 Emoji（写入 stdout）
    #[allow(dead_code)]
    pub fn draw_bug(x: u16, y: u16, emoji: &str) -> anyhow::Result<()> {
        let mut stdout = stdout();
        execute!(stdout, MoveTo(x, y), Print(emoji))?;
        Ok(())
    }

    /// 清空指定位置（写入空格，写入 stdout）
    pub fn clear_at(x: u16, y: u16) -> anyhow::Result<()> {
        let mut stdout = stdout();
        execute!(stdout, MoveTo(x, y), Print(" "))?;
        Ok(())
    }

    /// 刷新 stdout
    pub fn flush() -> anyhow::Result<()> {
        stdout().flush()?;
        Ok(())
    }

    /// 在指定位置写入 Emoji（写入任意 Write 对象，如终端设备文件）
    pub fn draw_bug_to<W: Write>(w: &mut W, x: u16, y: u16, emoji: &str) -> std::io::Result<()> {
        // 使用 ANSI 转义序列：\x1b[{row};{col}H{emoji}
        write!(w, "\x1b[{};{}H{}", y + 1, x + 1, emoji)
    }

    /// 在指定位置写入指定字符（写入任意 Write 对象，用于恢复被虫子覆盖的文字）
    pub fn clear_at_to<W: Write>(w: &mut W, x: u16, y: u16, ch: char) -> std::io::Result<()> {
        write!(w, "\x1b[{};{}H{}", y + 1, x + 1, ch)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
