//! 虫子动画引擎 — 屏幕覆盖模式
//!
//! 虫子是"漂浮在屏幕上"的视觉特效。覆盖文字时：
//!   1. 从 ScreenBuffer 保存原始字符
//!   2. 在终端上绘制虫子 Emoji
//! 虫子移走/消失时：
//!   1. 从 ScreenBuffer 恢复原始字符
//!   2. 写回终端

use crate::screen::ScreenBuffer;
use crate::terminal::Terminal;

use std::io::Write;
use std::time::{Duration, Instant};

/// 虫子 Emoji 池
const BUG_EMOJIS: &[&str] = &["🦟", "🪰", "🐝", "🕷️", "🦗"];

/// 单只虫子
#[derive(Debug, Clone)]
pub struct Bug {
    pub x: u16,
    pub y: u16,
    pub emoji: &'static str,
    pub birth: Instant,
    pub direction: i8, // -1: 向左, 1: 向右
    pub lifetime: Duration,
    /// 虫子在当前位置覆盖的原始字符（用于恢复）
    pub saved_char: Option<char>,
}

/// 虫子管理器
pub struct BugManager {
    bugs: Vec<Bug>,
    max_concurrent: usize,
}

impl BugManager {
    pub fn new(max_concurrent: usize, _cols: u16, _rows: u16) -> Self {
        Self {
            bugs: Vec::new(),
            max_concurrent,
        }
    }

    /// 触发一波虫子（根据匹配次数）
    pub fn trigger(&mut self, match_count: usize, lifetime: Duration, screen: &ScreenBuffer) {
        if match_count == 0 {
            return;
        }
        let available = self.max_concurrent.saturating_sub(self.bugs.len());
        if available == 0 {
            return;
        }
        let cols = screen.cols();
        let rows = screen.rows();
        if cols == 0 || rows == 0 {
            return;
        }

        let seed = Instant::now().elapsed().as_nanos() as u64;
        let mut rng = SimpleRng::new(seed);

        let mut to_spawn = 0;
        for _ in 0..match_count {
            let count = 2 + (rng.next() % 4) as usize; // 2~5
            to_spawn += count;
        }
        let to_spawn = to_spawn.min(available);

        for _ in 0..to_spawn {
            let y = (rng.next() % (rows / 2).max(1) as u64) as u16;
            let x = (rng.next() % cols.max(1) as u64) as u16;
            let emoji_idx = (rng.next() % BUG_EMOJIS.len() as u64) as usize;
            let direction = if rng.next() % 2 == 0 { 1 } else { -1 };

            // 不要立即保存字符——等 draw_to 时再保存（此时才真正覆盖）
            self.bugs.push(Bug {
                x,
                y,
                emoji: BUG_EMOJIS[emoji_idx],
                birth: Instant::now(),
                direction,
                lifetime,
                saved_char: None,
            });
        }
    }

    /// 更新虫子位置 & 移除超时虫子
    ///
    /// 返回 [(x, y, original_char)] 需恢复的位置列表
    pub fn update<W: Write>(
        &mut self,
        _dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<Vec<(u16, u16, char)>> {
        let now = Instant::now();
        let mut to_restore = Vec::new();

        let cols = screen.cols();
        let rows = screen.rows();

        // 第一步：恢复所有移走的虫子的原始字符
        for bug in &self.bugs {
            if let Some(_saved) = bug.saved_char {
                // 虫子还活着但需要恢复旧位置——在位置更新前做
                // 但我们还不知道新位置，所以先收集旧的
            }
        }

        // 更新位置、移除超时虫子
        self.bugs.retain(|bug| {
            let elapsed = now.duration_since(bug.birth);
            elapsed < bug.lifetime
        });

        for bug in &mut self.bugs {
            // 如果虫子之前有保存的字符，先恢复
            if let Some(saved) = bug.saved_char.take() {
                to_restore.push((bug.x, bug.y, saved));
                // 同时告诉 screen 这个位置不再被虫子持有
                screen.restore_char(bug.x, bug.y);
            }

            // 水平移动
            let new_x = bug.x as i32 + bug.direction as i32;
            if new_x >= 0 && new_x < cols as i32 {
                bug.x = new_x as u16;
            } else {
                bug.direction = -bug.direction;
            }

            // 随机上下浮动
            let seed = now.elapsed().as_nanos() as u64;
            let mut rng = SimpleRng::new(seed.wrapping_add(bug.x as u64));
            let dy = if rng.next() % 2 == 0 { 1i16 } else { -1i16 };
            let new_y = bug.y as i16 + dy;
            if new_y >= 0 && new_y < rows as i16 {
                bug.y = new_y as u16;
            }
        }

        Ok(to_restore)
    }

    /// 绘制所有虫子（先保存再覆盖）
    pub fn draw_to<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        for bug in &mut self.bugs {
            // 保存当前位置的原始字符
            let ch = screen.save_char(bug.x, bug.y);
            bug.saved_char = Some(ch);
            // 绘制虫子 Emoji 覆盖
            Terminal::draw_bug_to(dev, bug.x, bug.y, bug.emoji)?;
        }
        Ok(())
    }

    /// 恢复所有虫子的原始字符（退出时调用）
    pub fn clear_all<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        for bug in self.bugs.drain(..) {
            if let Some(ch) = bug.saved_char {
                // 写回原始字符
                Terminal::clear_at_to(dev, bug.x, bug.y, ch)?;
                screen.restore_char(bug.x, bug.y);
            }
        }
        Ok(())
    }

    /// 恢复指定位置的字符
    pub fn restore_positions<W: Write>(
        &self,
        dev: &mut W,
        positions: &[(u16, u16, char)],
    ) -> std::io::Result<()> {
        for &(x, y, ch) in positions {
            Terminal::clear_at_to(dev, x, y, ch)?;
        }
        Ok(())
    }
}

/// 简单的伪随机数生成器
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state >> 33
    }
}
