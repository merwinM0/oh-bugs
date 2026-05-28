use crossterm::cursor::{MoveTo, Show, Hide};
use crossterm::execute;
use crossterm::terminal::{self, size as terminal_size};
use crossterm::style::Print;
use std::io::{stdout, Write};

/// 终端管理器，负责原始模式进入/退出、光标操作
pub struct Terminal {
    original_raw_mode: bool,
}

impl Terminal {
    /// 进入原始模式，隐藏光标
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

    /// 获取终端大小（列，行）
    pub fn size() -> anyhow::Result<(u16, u16)> {
        Ok(terminal_size()?)
    }

    /// 在指定位置写入 Emoji
    pub fn draw_bug(x: u16, y: u16, emoji: &str) -> anyhow::Result<()> {
        let mut stdout = stdout();
        execute!(stdout, MoveTo(x, y), Print(emoji))?;
        Ok(())
    }

    /// 清空指定位置（写入空格）
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
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
