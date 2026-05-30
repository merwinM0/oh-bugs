//! 虫子动画引擎 — 屏幕覆盖模式
//!
//! 每只虫子通过 `saved_left` / `saved_right` **自己记住自己覆盖了什么内容**。
//!
//! 移动时：把 saved_* 恢复到旧位置 → 移动 → 保存新位置的内容到 saved_*
//! 死亡时：把 saved_* 恢复到当前位置
//!
//! ScreenBuffer 只提供纯文本追踪，不感知虫子。

use crate::screen::ScreenBuffer;
use crate::terminal::Terminal;

use std::io::Write;
use std::time::{Duration, Instant};

/// 虫子 Emoji 池（🦟🪰🐝🕷️🦗 在大多数终端中占 2 列宽度）
const BUG_EMOJIS: &[(&str, char)] = &[
    ("🦟", '🦟'),
    ("🪰", '🪰'),
    ("🐝", '🐝'),
    ("🕷️", '🕷'),
    ("🦗", '🦗'),
];

/// 被虫子覆盖的单元格信息
///
/// 记录原始字符及位置，用于后续恢复。
#[derive(Debug, Clone, Copy)]
pub struct SavedCell {
    /// 被覆盖的原始字符
    pub character: char,
    /// 位置（列）
    pub x: u16,
    /// 位置（行）
    pub y: u16,
}

impl SavedCell {
    /// 创建一个空白的 SavedCell（character = '\0' 表示未初始化）
    fn empty(x: u16, y: u16) -> Self {
        Self {
            character: '\0',
            x,
            y,
        }
    }

    /// 从 ScreenBuffer 读取并保存 (x, y) 处的字符
    fn capture(screen: &ScreenBuffer, x: u16, y: u16) -> Self {
        Self {
            character: screen.get_char(x, y),
            x,
            y,
        }
    }
}

/// 单只虫子
///
/// 虫子 Emoji 占 2 列宽度，覆盖 (x, y) 和 (x+1, y) 两个单元格。
/// 自己通过 `saved_left` / `saved_right` 记住自己覆盖了什么。
#[derive(Debug, Clone)]
pub struct Bug {
    /// 唯一标识符
    #[allow(dead_code)]
    pub id: u64,
    /// 当前位置（虫子左上角，左列）
    pub x: u16,
    pub y: u16,
    /// 虫子图案
    pub emoji: char,
    /// 创建时间
    pub birth: Instant,
    /// 水平移动方向：-1 向左，1 向右
    pub direction_x: i8,
    /// 垂直移动方向：-1 向上，1 向下
    pub direction_y: i8,
    /// 随机相位
    pub phase: u64,
    /// 左列保存的内容（位置 (x, y)）
    pub saved_left: SavedCell,
    /// 右列保存的内容（位置 (x+1, y)）
    pub saved_right: SavedCell,
    /// 生命周期
    pub lifetime: Duration,
}

/// 虫子管理器
pub struct BugManager {
    bugs: Vec<Bug>,
    max_concurrent: usize,
    next_id: u64,
    tick: u64,
}

impl BugManager {
    pub fn new(max_concurrent: usize, _cols: u16, _rows: u16) -> Self {
        Self {
            bugs: Vec::new(),
            max_concurrent,
            next_id: 1,
            tick: 0,
        }
    }

    /// 触发一波虫子
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
        if cols < 2 || rows == 0 {
            return;
        }

        let seed = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64)
            .wrapping_add(self.tick);
        let mut rng = SimpleRng::new(seed);
        self.tick = self.tick.wrapping_add(1);

        let mut to_spawn = 0;
        for _ in 0..match_count {
            let count = 2 + (rng.next() % 4) as usize;
            to_spawn += count;
        }
        let to_spawn = to_spawn.min(available);

        let base_ms = lifetime.as_millis() as u64;
        let half = 500u64.min(base_ms);
        let max_x = cols.saturating_sub(2).max(1);

        for _ in 0..to_spawn {
            let y = (rng.next() % (rows / 2).max(1) as u64) as u16;
            let x = (rng.next() % max_x as u64) as u16;
            let (_, emoji_char) = BUG_EMOJIS[rng.next() as usize % BUG_EMOJIS.len()];
            let id = self.next_id;
            self.next_id += 1;

            let direction_x = if rng.next() % 2 == 0 { 1 } else { -1 };
            let direction_y = if rng.next() % 2 == 0 { 1 } else { -1 };
            let ms = base_ms - half + (rng.next() % (half * 2 + 1));
            let bug_lifetime = Duration::from_millis(ms.max(500));
            let phase = rng.next();

            self.bugs.push(Bug {
                id,
                x,
                y,
                emoji: emoji_char,
                birth: Instant::now(),
                direction_x,
                direction_y,
                phase,
                saved_left: SavedCell::empty(x, y),
                saved_right: SavedCell::empty(x + 1, y),
                lifetime: bug_lifetime,
            });
        }
    }

    /// 每 tick 更新一次
    ///
    /// 每只虫子：
    ///   - 超时 → 把 saved_* 恢复到 old 位置，丢弃
    ///   - 存活 → 把 saved_* 恢复到 old 位置 → 移动 → 保存新位置到 saved_* → 绘制 emoji
    pub fn update<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        self.tick = self.tick.wrapping_add(1);
        let now = Instant::now();
        let cols = screen.cols();
        let rows = screen.rows();
        if cols < 2 || rows == 0 {
            self.bugs.clear();
            return Ok(());
        }

        let max_x = cols.saturating_sub(2).max(1);
        let safe_rows = rows.saturating_sub(2);
        let mut emoji_buf = [0u8; 4];
        let mut alive = Vec::with_capacity(self.bugs.len());

        for mut bug in self.bugs.drain(..) {
            let elapsed = now.duration_since(bug.birth);

            if elapsed >= bug.lifetime {
                // ── 虫子死亡 ─────────────────────────────────
                // 把 saved_* 恢复到它记住的位置（就是当前位置）
                if bug.saved_left.character != '\0' {
                    Terminal::clear_at_to(dev, bug.saved_left.x, bug.saved_left.y, bug.saved_left.character)?;
                    Terminal::clear_at_to(dev, bug.saved_right.x, bug.saved_right.y, bug.saved_right.character)?;
                }
                // 丢弃虫子
            } else {
                // ── 虫子存活 ─────────────────────────────────

                // 1) 把 saved_* 恢复到旧位置
                if bug.saved_left.character != '\0' {
                    Terminal::clear_at_to(dev, bug.saved_left.x, bug.saved_left.y, bug.saved_left.character)?;
                    Terminal::clear_at_to(dev, bug.saved_right.x, bug.saved_right.y, bug.saved_right.character)?;
                }

                // 2) 随机移动
                let mut rng = SimpleRng::new(
                    self.tick
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(bug.phase),
                );

                // 水平
                let dx = if bug.x < 3 && bug.direction_x == -1 {
                    bug.direction_x = 1;
                    1
                } else if bug.x >= max_x && bug.direction_x == 1 {
                    bug.direction_x = -1;
                    -1
                } else {
                    match rng.next() % 10 {
                        0..=5 => bug.direction_x as i32,
                        6..=8 => 0,
                        _ => -bug.direction_x as i32,
                    }
                };
                let new_x = bug.x as i32 + dx;
                if new_x <= 0 {
                    bug.x = 1;
                    bug.direction_x = 1;
                } else if new_x as u16 >= max_x {
                    bug.x = max_x;
                    bug.direction_x = -1;
                } else {
                    bug.x = new_x as u16;
                }

                // 垂直
                let dy = if bug.y < 2 && bug.direction_y == -1 {
                    bug.direction_y = 1;
                    1i16
                } else if bug.y >= safe_rows && bug.direction_y == 1 {
                    bug.direction_y = -1;
                    -1i16
                } else {
                    match rng.next() % 10 {
                        0..=5 => bug.direction_y as i16,
                        6..=8 => 0i16,
                        _ => -bug.direction_y as i16,
                    }
                };
                let new_y = bug.y as i16 + dy;
                if new_y <= 0 {
                    bug.y = 1;
                    bug.direction_y = 1;
                } else if new_y as u16 >= rows {
                    bug.y = safe_rows.max(1);
                    bug.direction_y = -1;
                } else {
                    bug.y = new_y as u16;
                }

                // 3) 保存新位置的内容到 saved_*
                bug.saved_left = SavedCell::capture(screen, bug.x, bug.y);
                bug.saved_right = SavedCell::capture(screen, bug.x + 1, bug.y);

                // 4) 在新位置绘制 Emoji
                let emoji_str = bug.emoji.encode_utf8(&mut emoji_buf);
                Terminal::draw_bug_to(dev, bug.x, bug.y, emoji_str)?;

                alive.push(bug);
            }
        }

        self.bugs = alive;
        Ok(())
    }

    /// 为刚创建的虫子（saved_* 尚未捕获）读取屏幕并绘制 emoji
    pub fn draw_to<W: Write>(
        &mut self,
        dev: &mut W,
        screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        let mut emoji_buf = [0u8; 4];
        for bug in &mut self.bugs {
            if bug.saved_left.character == '\0' {
                bug.saved_left = SavedCell::capture(screen, bug.x, bug.y);
                bug.saved_right = SavedCell::capture(screen, bug.x + 1, bug.y);
                let emoji_str = bug.emoji.encode_utf8(&mut emoji_buf);
                Terminal::draw_bug_to(dev, bug.x, bug.y, emoji_str)?;
            }
        }
        Ok(())
    }

    /// 恢复所有虫子的原始字符（退出时调用）
    ///
    /// 每只虫子把 saved_* 恢复到它记住的位置（就是最后覆盖的位置）。
    pub fn clear_all<W: Write>(
        &mut self,
        dev: &mut W,
        _screen: &mut ScreenBuffer,
    ) -> std::io::Result<()> {
        for bug in self.bugs.drain(..) {
            if bug.saved_left.character != '\0' {
                Terminal::clear_at_to(dev, bug.saved_left.x, bug.saved_left.y, bug.saved_left.character)?;
                Terminal::clear_at_to(dev, bug.saved_right.x, bug.saved_right.y, bug.saved_right.character)?;
            }
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
