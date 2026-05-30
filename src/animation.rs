//! 虫子动画引擎 — 屏幕覆盖模式
//!
//! ## 方案 B（当前实现）
//!
//! 虫子不保存原始字符。移动/死亡时直接从 ScreenBuffer 实时读取最新字符来恢复：
//!
//! 1. 虫子出现在 (x, y)：直接绘制 emoji（ScreenBuffer 不受影响）
//! 2. 虫子移动：从 ScreenBuffer 读 (old_x, old_y)→写回终端 → 更新 old_x/y → 在新位置绘制 emoji
//! 3. 虫子死亡：从 ScreenBuffer 读 (old_x, old_y)→写回终端
//!
//! 无论 PTY 输出在虫子存活期间如何变化，ScreenBuffer 始终追踪最新内容，
//! 虫子恢复时永远拿到的是最新字符，不会写回过时数据。

use crate::screen::ScreenBuffer;
use crate::terminal::Terminal;

use std::collections::HashSet;
use std::io::Write;
use std::time::{Duration, Instant};

const BUG_EMOJIS: &[(&str, char)] = &[
    ("🦟", '🦟'),
    ("🪰", '🪰'),
    ("🐝", '🐝'),
    ("🕷️", '🕷'),
    ("🦗", '🦗'),
];

/// 单只虫子 — 不保存原始字符，始终从 ScreenBuffer 实时读取
#[derive(Debug, Clone)]
pub struct Bug {
    #[allow(dead_code)]
    pub id: u64,
    /// 当前位置
    pub x: u16,
    pub y: u16,
    /// 上一帧位置（用于恢复）
    pub old_x: u16,
    pub old_y: u16,
    pub emoji: char,
    pub birth: Instant,
    pub direction_x: i8,
    pub direction_y: i8,
    pub phase: u64,
    pub lifetime: Duration,
}

pub struct BugManager {
    bugs: Vec<Bug>,
    max_concurrent: usize,
    next_id: u64,
    tick: u64,
}

impl BugManager {
    pub fn new(max_concurrent: usize, _cols: u16, _rows: u16) -> Self {
        Self { bugs: Vec::new(), max_concurrent, next_id: 1, tick: 0 }
    }

    pub fn trigger(&mut self, match_count: usize, lifetime: Duration, screen: &ScreenBuffer) {
        if match_count == 0 { return; }
        let available = self.max_concurrent.saturating_sub(self.bugs.len());
        if available == 0 { return; }
        let cols = screen.cols();
        let rows = screen.rows();
        if cols < 2 || rows == 0 { return; }

        let seed = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64)
            .wrapping_add(self.tick);
        let mut rng = SimpleRng::new(seed);
        self.tick = self.tick.wrapping_add(1);

        // 固定生成 8 只虫子，不从 error 次数累加
        let to_spawn = 8usize.min(available);
        let base_ms = lifetime.as_millis() as u64;
        let half = 500u64.min(base_ms);
        let max_x = cols.saturating_sub(2).max(1);

        let mut occupied: HashSet<(u16, u16)> = HashSet::new();
        for bug in &self.bugs {
            occupied.insert((bug.x, bug.y));
            occupied.insert((bug.x + 1, bug.y));
        }

        for _ in 0..to_spawn {
            let (mut y, mut x);
            let mut attempt = 0u32;
            loop {
                y = (rng.next() % (rows / 2).max(1) as u64) as u16;
                x = (rng.next() % max_x as u64) as u16;
                if !occupied.contains(&(x, y)) && !occupied.contains(&(x + 1, y)) { break; }
                attempt += 1;
                if attempt >= 100 { break; }
            }
            occupied.insert((x, y));
            occupied.insert((x + 1, y));
            let (_, emoji_char) = BUG_EMOJIS[rng.next() as usize % BUG_EMOJIS.len()];
            let id = self.next_id;
            self.next_id += 1;
            let direction_x = if rng.next() % 2 == 0 { 1 } else { -1 };
            let direction_y = if rng.next() % 2 == 0 { 1 } else { -1 };
            let ms = base_ms - half + (rng.next() % (half * 2 + 1));
            let phase = rng.next();

            self.bugs.push(Bug {
                id, x, y,
                old_x: x, old_y: y,
                emoji: emoji_char, birth: Instant::now(),
                direction_x, direction_y, phase,
                lifetime: Duration::from_millis(ms.max(500)),
            });
        }
    }

    // ── restore helper: 从 ScreenBuffer 实时读取 (x,y) 和 (x+1,y)，一次写入两个字符 ──
    /// 用 clear_bug_at_to 合并写入，避免终端将第 2 列视为"宽字符延续"而丢弃写入。
    fn restore_at<W: Write>(dev: &mut W, screen: &ScreenBuffer, x: u16, y: u16) -> std::io::Result<()> {
        let ch_left = screen.get_char(x, y);
        let ch_right = screen.get_char(x + 1, y);
        Terminal::clear_bug_at_to(dev, x, y, ch_left, ch_right)
    }

    /// 每 tick 更新一次
    ///
    /// 每只虫子（方案 B）：
    ///   1. 死亡 → 从 ScreenBuffer 实时读 (old_x, old_y) → 写回终端，丢弃
    ///   2. 存活 → 先移动，再恢复旧位置 → 记录 old = 当前位置 → 在新位置绘制 emoji
    ///
    /// "先移动再恢复"：保存 old 位 → 计算新位 → 恢复上帧的 old 位（此时旧位置已清空）
    /// → 绘制新位。这样不同虫子的恢复/绘制区域不会重叠。
    ///
    /// 注意：不在此处做 save/restore。调用方（daemon）负责在批次结束后
    /// 用 ScreenBuffer 的 cursor 位置显式 CUP 恢复光标。
    pub fn update<W: Write>(&mut self, dev: &mut W, screen: &mut ScreenBuffer) -> std::io::Result<()> {
        self.tick = self.tick.wrapping_add(1);
        let now = Instant::now();
        let cols = screen.cols();
        let rows = screen.rows();
        if cols < 2 || rows == 0 { self.bugs.clear(); return Ok(()); }

        let max_x = cols.saturating_sub(2).max(1);
        let safe_rows = rows.saturating_sub(2);
        let mut emoji_buf = [0u8; 4];
        let mut alive = Vec::with_capacity(self.bugs.len());

        for mut bug in self.bugs.drain(..) {
            if now.duration_since(bug.birth) >= bug.lifetime {
                // ── 死亡：从 ScreenBuffer 实时读最新字符恢复 ──
                Self::restore_at(dev, screen, bug.old_x, bug.old_y)?;
            } else {
                // ── 存活：保存上帧位置 → 先移动 → 再恢复上帧旧位 → 绘制新位 ──
                let restore_x = bug.old_x;
                let restore_y = bug.old_y;

                let mut rng = SimpleRng::new(self.tick.wrapping_mul(6364136223846793005).wrapping_add(bug.phase));

                let dx = if bug.x < 3 && bug.direction_x == -1 { bug.direction_x = 1; 1 }
                else if bug.x >= max_x && bug.direction_x == 1 { bug.direction_x = -1; -1 }
                else { match rng.next() % 10 { 0..=5 => bug.direction_x as i32, 6..=8 => 0, _ => -bug.direction_x as i32 } };
                let new_x = bug.x as i32 + dx;
                if new_x <= 0 { bug.x = 1; bug.direction_x = 1; }
                else if new_x as u16 >= max_x { bug.x = max_x; bug.direction_x = -1; }
                else { bug.x = new_x as u16; }

                let dy = if bug.y < 2 && bug.direction_y == -1 { bug.direction_y = 1; 1i16 }
                else if bug.y >= safe_rows && bug.direction_y == 1 { bug.direction_y = -1; -1i16 }
                else { match rng.next() % 10 { 0..=5 => bug.direction_y as i16, 6..=8 => 0i16, _ => -bug.direction_y as i16 } };
                let new_y = bug.y as i16 + dy;
                if new_y <= 0 { bug.y = 1; bug.direction_y = 1; }
                else if new_y as u16 >= rows { bug.y = safe_rows.max(1); bug.direction_y = -1; }
                else { bug.y = new_y as u16; }

                // 先恢复上帧旧位置（在移动之后才执行，但坐标是移动前保存的）
                Self::restore_at(dev, screen, restore_x, restore_y)?;

                // 记录当前位置为下一帧的 old 位
                bug.old_x = bug.x;
                bug.old_y = bug.y;

                // 在新位置绘制 emoji
                let emoji_str = bug.emoji.encode_utf8(&mut emoji_buf);
                Terminal::draw_bug_to(dev, bug.x, bug.y, emoji_str)?;

                alive.push(bug);
            }
        }

        self.bugs = alive;
        Ok(())
    }

    /// 为刚创建的虫子绘制 emoji。方案 B：不保存任何字符。
    /// 注意：调用方（daemon）负责在批次结束后 CUP 恢复光标。
    pub fn draw_to<W: Write>(&mut self, dev: &mut W, _screen: &mut ScreenBuffer) -> std::io::Result<()> {
        let mut emoji_buf = [0u8; 4];
        for bug in &self.bugs {
            // 如果 old == current，说明刚创建还未在终端上画过 emoji
            if bug.old_x == bug.x && bug.old_y == bug.y {
                let emoji_str = bug.emoji.encode_utf8(&mut emoji_buf);
                Terminal::draw_bug_to(dev, bug.x, bug.y, emoji_str)?;
            }
        }
        Ok(())
    }

    /// 退出时恢复所有虫子的位置
    /// 注意：调用方（daemon）负责在批次结束后 CUP 恢复光标。
    pub fn clear_all<W: Write>(&mut self, dev: &mut W, screen: &mut ScreenBuffer) -> std::io::Result<()> {
        for bug in self.bugs.drain(..) {
            Self::restore_at(dev, screen, bug.old_x, bug.old_y)?;
        }
        Ok(())
    }
}

struct SimpleRng { state: u64 }
impl SimpleRng {
    fn new(seed: u64) -> Self { Self { state: if seed == 0 { 1 } else { seed } } }
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.state >> 33
    }
}
